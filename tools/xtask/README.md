# xtask

Newport's build, packaging, release and test tooling, written in Rust and run
with `cargo xtask <command>` from the repository root. The alias lives in the
committed `.cargo/config.toml`.

Maintainer guides: [Testing and upgrade acceptance](testing.md),
[Release publishing](releases.md), and [RSA dependency review](security-review.md).
User documentation starts at the [Newport user guide](../../docs/README.md).

The crate is outside the root workspace and has its own `Cargo.lock` and
`target/` directory. Its test-container dependencies would otherwise enter the
desktop dependency graph, and it always builds for the host while the desktop
crates may build for another target.

## Commands

### Develop

| Command | Does |
| --- | --- |
| `cargo xtask dev [--remote-apps] [-- <args>…]` | Runs the desktop app: builds the Linux agents with Docker on macOS when missing or stale; Windows needs the resources described below. Stages grammars when stale and PDFium on Windows, then runs `cargo run -p newport-app`. Without `NEWPORT_DATA_DIR` it uses the disposable profile `target/dev-profile`. On macOS the app's output streams to the terminal, and a development Newport still running from the same bundle is quit first. `--remote-apps` compiles Remote apps in (see below). |
| `cargo xtask build [--release] [--target <triple>] [--remote-apps]` | The same resources, then `cargo build -p newport-app` in that environment. |
| `cargo xtask grammars [--release] [--target <triple>]` | Builds every tree-sitter grammar library and stages it in `<target>/<profile>/grammars`. `dev` and `build` do this when the staged libraries are missing or older than `crates/newport-grammars`, `Cargo.lock` or the language list. |
| `cargo xtask pdfium [--release] [--target <triple>]` | Downloads, verifies and stages PDFium for Windows. |
| `cargo xtask agents [--if-changed \| --arch x86_64\|aarch64]` | On macOS or Linux, builds the Linux agent binaries in Docker; `--if-changed` only when they are missing or older than `tools/agent`'s sources (as `dev` and `build` do), `--arch` only that architecture (CI builds each natively). Windows requires prebuilt resources. With `NEWPORT_PREBUILT=1`, `agents` verifies downloaded ones instead of building. |
| `cargo xtask remote-agents [--arch x86_64\|aarch64]…` | Builds the standalone Remote apps agent archives in `target/remote-agent`. Desktop builds don't use them (see [Remote apps](#remote-apps)). |
| `cargo xtask artifact record\|verify <dir>` | Records or checks `newport-build.json`: per-file SHA-256 and the commit. |
| `cargo xtask dev-run <binary> [<args>…]` | macOS cargo runner. Bundles, signs and launches `newport` through LaunchServices; other binaries run directly. |

The terminal builds directly with Cargo using the pinned Rust toolchain.
Use `cargo xtask dev`/`build` to stage the app resources as well.

### Windows agent resources

A Windows desktop build embeds both Linux agent architectures. Prepare them
before the first desktop build or check; installing Docker on Windows does not
make `cargo xtask agents` build them.

1. Run `git rev-parse HEAD` in your checkout to identify its commit.
2. Open the public repository's [CI runs](https://github.com/ruiyangke/newport/actions/workflows/ci.yml)
   and select a successful run for that exact commit. Download its **linux-agents**
   artifact from the run's **Artifacts** section. GitHub requires signing in to
   download workflow artifacts; access to the private Remote apps repository is
   not required.
3. Extract the ZIP directly into `resources/agents`, including `newport-build.json`
   and both `newport-agent-*` binaries. Do not add a nested `linux-agents` folder
   or mix files from different runs. Preserve any existing resource directory
   separately before replacing it.

For a fresh checkout, run this in PowerShell from the repository root, adjusting
the downloaded ZIP path if needed:

```powershell
Expand-Archive -LiteralPath "$HOME\Downloads\linux-agents.zip" -DestinationPath resources\agents -ErrorAction Stop
cargo xtask artifact verify resources/agents
if ($LASTEXITCODE -ne 0) { throw "Agent artifact verification failed" }
$env:NEWPORT_PREBUILT = "1"
```

Verification checks the artifact's commit against your checkout and every file
against its recorded hash. Keep `NEWPORT_PREBUILT=1` in the shell used for the
desktop commands. These are development resources, not production-signed
release artifacts. If the matching artifact has expired or you are changing
agent sources, build the agents with `cargo xtask agents` on macOS or Linux with
Docker, then run `cargo xtask artifact record resources/agents` there and transfer
that complete directory. Use matching checkouts, including any local agent
edits, on both machines and verify the transferred directory on Windows before
building. The manifest verifies the recorded commit and payloads; it does not
compare uncommitted source edits.

### Check and test

| Command | Does |
| --- | --- |
| `cargo xtask check [rust\|agent\|xtask]` | Formatting, Clippy and tests. With no argument, all three. |
| `cargo xtask test desktop [--headless]` | Profile, preferences and metrics persistence, resource staging, the desktop app's lifecycle. |
| `cargo xtask test ssh [<test filter>]` | Disposable SSH fixture: terminal, file and RPC operations; also checks the fixture command parser. |
| `cargo xtask test clipboard [--linux-peer <launcher>]` | macOS clipboard checks on disposable named pasteboards; the optional peer adds composed Linux clipboard checks. |
| `cargo xtask test remote [--filter <test>]` | Testcontainers with real OpenSSH and remote services. |
| `cargo xtask test https` | Agent Git over HTTPS against a local `git http-backend`. |
| `cargo xtask test runtime [--arch x86_64\|aarch64]` | Agent runtime checks inside the Alpine image, for both architectures or the one named. |
| `cargo xtask test git-contract [--output <path>]` | Real agent Git responses through the app's validators. |
| `cargo xtask test git-cli-contract [--output <path>]` | Agent Git CLI responses through the app's validators. |
| `cargo xtask test benchmark --output <path> [--agent <path>] [--before <rev>] [--after <rev>] [--size <size>]… [--rounds <n>] [--latency-ms <ms>]` | Git benchmark; `--output` is required and `--rounds` defaults to 3. |
| `cargo xtask test keychain` | macOS: signs a test bundle and runs the protected Keychain test. |

The SSH lane selects tests assigned to its adverse server, which recognizes a
bounded command set. Tests needing real Linux installation use `test remote`.
An ignored test without a fixture assignment fails selection instead of silently
being omitted.

`test clipboard` builds the public `clipboard_owner_check` helper and runs the
native clipboard tests without using the general pasteboard or a desktop profile.
For the composed clipboard tests, supply `--linux-peer` as the path to an
executable local Docker fixture launcher, and set `NEWPORT_AGENT_RESOURCE_DIR` to
a verified full agent export. The launcher must use that export in its disposable
Linux display/agent fixture and speak the JSON peer protocol exercised by
`clipboard_export_copy_live_tests.rs`. The command passes the launcher through
`NEWPORT_CLIPBOARD_LINUX_PEER`; it does not discover a server, acquire a license,
or build private source. Missing prerequisites fail before running tests.

The full-product install test also needs that full export and real Linux OpenSSH:

```sh
NEWPORT_AGENT_RESOURCE_DIR=/path/to/full-export cargo xtask test remote --filter agent_full_product_tests::
```

Full exports must match the checkout's artifact manifest and source provenance;
CI additionally requires a clean production export for the pinned source pair.
These composed lanes qualify private-product behavior separately from the public
core SSH and native clipboard jobs.

### Package and release

| Command | Does |
| --- | --- |
| `cargo xtask package [--debug] [--skip-build] [--target <triple>] [--nsis] [--remote-apps]` | Stages the app and verifies it. macOS: `Newport.app` plus a zip. Windows: a zip, and with `--nsis` the installer. `--remote-apps` (macOS) compiles Remote apps in. |
| `cargo xtask release verify` | Checks the tag against every version and the checkout. A release builds everything it ships from the tagged commit, without depending on a CI run. |
| `cargo xtask release setup-signing` | CI: writes the certificate and profile and creates a temporary keychain. |
| `cargo xtask release macos [--skip-build] [--remote-apps]` | Developer ID signing, notarization, stapling, and the signed update tarball and manifest fragment. |
| `cargo xtask release windows-update <installer> --arch <arch> --app-executable <exe> --output <dir>` | Signed Windows update payload and manifest fragment. |
| `cargo xtask release merge <latest.json> <fragment>… --version <v> --output <path>` | Merges the fragments, verifies every signature, and writes `SHA256SUMS`. |
| `cargo xtask release sign-needle` | Verifies `resources/agents` and signs the agent release. |
| `cargo xtask release sign-agent <dir>` | Verifies a full agent export (`artifact verify`), signs it for this release, verifies the signature and records the directory again. |
| `cargo xtask release verify-packages <latest.json> --assets <dir>` | Verifies all three update targets against the production public key, signed version and filename before distribution. Accepts existing signed manifests for migration without fetching their URLs. |
| `cargo xtask release deploy <dir> [--dry-run] [--bucket <name>]` | Verifies and publishes the complete distribution to Cloudflare R2, then conditionally advances the update feed. Uses bucket-scoped S3 credentials; no website checkout or deployment. |

Signing uses `newport-release` as a library. The release workflow builds the
xtask binary in a step without secrets and runs that binary with them; commands
that build the desktop remove the signing secrets from the child environment.
The workflow uploads verified packages and release metadata directly to Cloudflare
R2, and retains a `release-<tag>` artifact. The independently deployed website
reads the feed from R2 at request time. See [the release guide](releases.md), including the
transition feed required by older installed builds.

## Local signing

On macOS, `cargo xtask dev` (or `cargo run -p newport-app`)
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
`cargo add`, and GPUI is patched to this lockfile's revision on the command
line. A guard takes the crates out of `Cargo.toml` and `Cargo.lock` again when
the build ends, fails or is interrupted with Ctrl-C; a build that was killed
outright is cleaned up by the next `dev`, `build` or `test`. The checkout never
keeps a reference to the private crates.

A Remote apps build bundles the full agent (core and Remote apps) instead of
the core agent, from an export directory: `newport-agent-<arch>`, its
`newport-agent-<arch>.product.json`, provenance and an artifact manifest for
both architectures. `NEWPORT_AGENT_RESOURCE_DIR` names it. Without it, a local
build runs newport-remote's `scripts/build-unified-agent.py` (Docker, from this
checkout's agent sources) and `scripts/export-unified-agent.py`, and keeps the
export in newport-remote's `target/unified-agent/desktop-resources`; with
`NEWPORT_PREBUILT=1` it uses that export as it is. Every export is checked
against this commit and the newport-remote commit before it is bundled.

`package --remote-apps` and `release macos --remote-apps` do the same for
distributions; Windows has no Remote apps yet.

Public CI checks the core edition without private dependencies. Remote apps
tests and native acceptance are [separate checks](testing.md#remote-apps-acceptance)
for maintainers with access to newport-remote.

Releases build the newport-remote commit in `.github/newport-remote-revision`;
a local build says when its checkout is at another one. After changing
newport-remote, push it and put its commit there.
The release workflow checks it out with the `NEWPORT_REMOTE_DEPLOY_KEY` secret,
a read-only deploy key of that repository, and builds everything from the
tagged commit, never from a CI run:

1. "Remote apps: agent" builds the full agent for each architecture in Docker
   on a runner of that architecture.
2. "Remote apps: sign agent" exports both with `export-unified-agent.py
   --production`, verifies the export and signs it with `cargo xtask release
   sign-agent`.
3. "Desktop: macOS" bundles that signed export through
   `NEWPORT_AGENT_RESOURCE_DIR` with `NEWPORT_PREBUILT=1`, and `release macos
   --remote-apps` checks that the packaged app embeds exactly that export
   before Developer ID signing and notarization.

## Not in xtask

- `tests/agent-runtime/private-directory.sh`, `tests/remote/start.sh` and
  `tests/remote/docker/start.sh` run inside containers.
- `tools/xtask/runner.sh` is the cargo runner shim: Cargo needs a runner that
  exists before anything is built.
- `scripts/test-windows-activation.ps1` is a manual Windows desktop check.
