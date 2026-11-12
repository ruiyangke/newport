# xtask

Newport's build, packaging, release and test tooling, written in Rust and run
with `cargo xtask <command>` from the repository root. The alias lives in the
committed `.cargo/config.toml`.

Maintainer guides: [Testing and upgrade acceptance](testing.md) and
[Release publishing](releases.md).
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
| `cargo xtask patched` | Generates the patched dependencies: each crate pinned in `patches/patched.toml` (and `remote/patches/patched.toml`), downloaded from crates.io, checked against its SHA-256 and patched with `patches/<name>.patch` into `patches/<name>/`, which Git ignores. Every other `cargo xtask` command does this first; run it before calling `cargo` directly. |
| `cargo xtask patched --save <crate>` | Writes `<crate>`'s patch from the edits made to its generated source. |
| `cargo xtask agents [--if-changed \| --arch x86_64\|aarch64]` | On macOS or Linux, builds the Linux agent binaries in Docker; `--if-changed` only when they are missing or older than `crates/newport-agent`'s sources (as `dev` and `build` do), `--arch` only that architecture (CI builds each natively). Windows requires prebuilt resources. With `NEWPORT_PREBUILT=1`, `agents` verifies downloaded ones instead of building. |
| `cargo xtask agents --remote-apps` | Builds the Full agent (core and Remote apps) for both architectures from this checkout and `remote/` as they are, exports it and keeps the export in `remote/target/unified-agent/desktop-resources`, as a Remote apps `dev` or `build` does when it has none. |
| `cargo xtask full-agent build [--production] [--arch x86_64\|aarch64] [--output-dir <dir>]` | Builds the Full agent in Docker into `<dir>/<arch>` (default `remote/target/unified-agent`), with the `tools/static-agent` builder in `remote/`. It reads a snapshot laid out as the checkout: with `--production`, `git archive` of HEAD and of `remote/` at its tag named like HEAD's release tag, which needs both checkouts clean and `remote/` at that tag; otherwise both working trees, marked dirty. |
| `cargo xtask full-agent export [--production] [--build-dir <dir>] --output <new dir>` | Checks both architectures' builds (ELF architecture, product identity, export protocol policy, matching provenance and notices) and writes the export: binaries, identities, the allowlisted notices, `full-agent-provenance.json` and `newport-build.json`. `--production` requires production builds of HEAD and of `remote/` at its tag of the same name. |
| `cargo xtask artifact record\|verify <dir>` | Records or checks `newport-build.json`: per-file SHA-256 and the commit. |
| `cargo xtask dev-run <binary> [<args>…]` | macOS cargo runner. Bundles, signs and launches `newport` through LaunchServices; other binaries run directly. |
| `cargo xtask trace <protocol> <message> [<capture>]` | Prints a captured channel's length-delimited frames as the Protocol Buffers JSON mapping, one message per line, with `bytes` fields as their length and secrets redacted. `<protocol>` is the protocol whose schemas the channel uses: `newport-protocol`, `remote-apps` (compiled from the `remote/` checkout), or the path of an encoded `FileDescriptorSet`; `<message>` is the channel's fully qualified top-level message; the capture defaults to standard input. A frame that doesn't decode prints as an `@error` line with its index and length, and the trace goes on. |
| `cargo xtask protocol-reference [--check]` | Generates the [wire and handshake reference](../../crates/newport-protocol/REFERENCE.md) from the shared crate's embedded protox descriptors, including schema comments. `--check` reports stale documentation without rewriting it. Uses the locked, cached Cargo dependencies offline; no system `protoc` or application resources are needed. |

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
| `cargo xtask check rust -p <package> [-p <package>…]` | Keep workspace formatting and isolated semantic-contract checks; limit Clippy and tests to the selected desktop packages. Useful when rebuilding the GUI would exceed the available disk budget. This is a scoped check, not the full workspace gate. |
| `cargo xtask fmt -- [cargo fmt args]` | Format Rust sources, including another workspace selected with `--manifest-path`. |
| `cargo xtask test rust [cargo test args]` | Run Rust tests through the test tooling. Use `--locked` for an unchanged dependency graph; `--manifest-path` selects a separate workspace's own tests and dev-dependencies. |
| `cargo xtask test desktop [--headless]` | Profile, preferences and metrics persistence, resource staging, the desktop app's lifecycle. |
| `cargo xtask test ssh [<test filter>]` | Disposable SSH fixture: terminal, file and RPC operations; also checks the fixture command parser. |
| `cargo xtask test clipboard [--linux-peer <launcher>]` | macOS clipboard checks on disposable named pasteboards; the optional peer adds composed Linux clipboard checks. |
| `cargo xtask test clipboard-windows --help` | Run one native Windows clipboard fixture using an explicit test executable and SHA-256, a fresh noninteractive window station, and a fresh evidence directory. Includes static fixtures and separate-process stalled-owner probes. |
| `cargo xtask test windows-window --gpui-source <absolute checkout> --output-directory <fresh evidence dir>` | Builds the real GPUI `window_lifecycle` example and runs it as an interactive Windows user with a temporary profile, deadline and exact-process cleanup. Records source/artifact hashes, Cargo output and native JSON evidence. |
| `cargo xtask test windows-input --gpui-source <absolute checkout> --output-directory <fresh evidence dir>` | Builds and runs the real GPUI `input_lifecycle` example with the same isolated profile and cleanup checks. Records native input observations and unavailable layout/IME coverage. |
| `cargo xtask test windows-provider-window --provider-source <absolute remote/ checkout> --gpui-source <absolute checkout> --gpui-config <absolute Cargo config> --output-directory <fresh evidence dir> --newport-profile <running isolated Newport profile>` | Builds the licensed `windows_window_contract` devtools bin in `remote/` with `desktop`. Requests and verifies a grant through Newport's local IPC before opening windows; checks provider geometry/actions and cleanup. Keep the signed-in app running in the same Windows logon. The existing app profile is separate from scratch and is never removed or copied. |
| `cargo xtask test windows-provider-input --provider-source <absolute remote/ checkout> --gpui-source <absolute checkout> --gpui-config <absolute Cargo config> --output-directory <fresh evidence dir> --newport-profile <running isolated Newport profile>` | Runs the licensed `windows_input_contract` devtools bin through the same IPC and cleanup path. Both provider commands require exactly one explicit license source: `--newport-profile`, or `--development-license` to use `NEWPORT_REMOTE_LICENSE_TOKEN`. No grant is printed, and a refused profile grant never falls back. The receipts retain scope and unavailable native coverage. |
| `cargo xtask test windows-drag --provider-source <absolute remote/ checkout> --gpui-source <absolute checkout> --gpui-config <absolute Cargo config> --output-directory <fresh evidence dir>` | Builds the `windows_drag_contract` devtools bin with the prepared lockfile and explicit GPUI overrides. Verifies one local GPUI identity, COM resource/storage cleanup and unchanged desktop state. This contract fixture does not establish user-driven or remote-app drag acceptance. |
| `cargo xtask test windows-drag-gesture --provider-source <absolute remote/ checkout> --gpui-source <absolute checkout> --gpui-config <absolute Cargo config> --output-directory <fresh evidence dir> --mode <cancel-mode\|release-capture>` | Builds `windows_drag_gesture` and runs the selected native cancellation experiment as a normal interactive Windows user. Wait for `ready-for-real-press-and-hold` in `fixture.stderr.log`, then hold the owned probe window for three seconds without moving. Requires cancellation while the button remains held, mode-specific native evidence and complete cleanup. Each mode needs fresh evidence. Records `productionDragQualified: false`; passing does not qualify GPUI reentry, successful drops or production drag integration. |
| `cargo xtask test windows-file-drop --provider-source <absolute remote/ checkout> --gpui-source <absolute checkout> --gpui-config <absolute Cargo config> --output-directory <fresh evidence dir> --scenario <plain\|ctrl\|shift> [--timeout-seconds 120]` | Builds `windows_file_drop` and runs it as a normal interactive Windows user. Read the flushed `ready` JSON line in `fixture.stdout.log`, open its owned `sample_directory` in Explorer, and perform one requested gesture into the reported receiving window. Checks the final client point, unchanged sample contents and clipboard identity, native cleanup and profile removal. Retains logs and preserves temporary files on failure. The scenario labels the manual gesture; it never establishes a native copy/move effect. Records `productionFileDropQualified: false`; no remote transfer or production drag capability is qualified. |
| `cargo xtask test windows-drop-extraction --provider-source <absolute remote/ checkout> --gpui-source <absolute checkout> --gpui-config <absolute Cargo config> --output-directory <fresh evidence dir>` | Runs the owned COM file extraction experiment with normal, canceled and stalled sources. Records exact child cleanup separately from proxy teardown, and preserves its temporary profile as evidence. Completion does not certify arbitrary COM objects or production UI responsiveness. |
| `cargo xtask test remote [--filter <test>]` | Testcontainers with real OpenSSH and remote services. |
| `cargo xtask test https` | Agent Git over HTTPS against a local `git http-backend`. |
| `cargo xtask test runtime [--arch x86_64\|aarch64]` | Agent runtime checks inside the Alpine image, for both architectures or the one named. |
| `cargo xtask test discovery --container <name> --agent <absolute binary> --helper-tests <absolute test binary> --unit-tests <absolute test binary> [--monitor-tests <absolute test binary>] [--elevation-unavailable] [--stalled-project-directory <absolute container path>]` | Runs prebuilt discovery unit and real-helper tests against a Core or Full agent as `newport` in an existing disposable Linux container. Use `--ssh <user@host> --ssh-identity <absolute key>` instead of `--container` for an isolated Linux VM; SSH retains existing host-key verification. `--systemd-tests <absolute test binary>` also runs system and user socket activation checks, creating and removing unique temporary runtime units. Normally requires noninteractive sudo; the holder test starts its own finite root-owned listener. `--elevation-unavailable` instead verifies missing/denied sudo and ordinary helper operation. The optional `--monitor-tests` runs the real monitor refresh/holder round trip using the same agent. An explicitly supplied `--stalled-project-directory` enables the filesystem deadline/cancellation test; it requires a disposable FUSE fixture whose lookup waits for its sibling `.release` marker and records a `.requested` marker. The runner does not mount or configure that fixture. Removes its temporary executables after success or failure. Build all supplied executables from the same tree for the container's architecture/libc with `cargo xtask test rust --manifest-path crates/newport-agent/Cargo.toml --locked --target <triple> --lib --test discovery_helper --test discovery_monitor --test discovery_systemd --no-run`. |
| `cargo xtask test git-contract [--output <path>]` | Real agent Git responses through the app's validators. |
| `cargo xtask test git-cli-contract [--output <path>]` | Agent Git CLI responses through the app's validators. |
| `cargo xtask test benchmark --output <path> [--agent <path>] [--before <rev>] [--after <rev>] [--size <size>]… [--rounds <n>] [--latency-ms <ms>]` | Git benchmark; `--output` is required and `--rounds` defaults to 3. |
| `cargo xtask test keychain` | macOS: signs a test bundle and runs the protected Keychain test. |

The SSH lane selects tests assigned to its adverse server, which recognizes a
bounded command set. Tests needing real Linux installation use `test remote`.
An ignored test without a fixture assignment fails selection instead of silently
being omitted.

`test remote` labels each fixture image with a digest of its build context and
build arguments, and builds it again only when they change. CI caches the
labelled images between runs.

The discovery helper suite uses live listeners to check executable names after
rename and exec, separate socket identities and holders with `SO_REUSEPORT`, and
socket retention when a non-dumpable process hides its descriptors. The last case
requires an unprivileged account without ptrace capability; `test discovery`
enables it explicitly and verifies that descriptor access is actually denied.
The disposable holder processes have finite lifetimes and are reaped on failure.

Discovery load qualification adds `--load-tests <absolute benchmark executable>`
to `test discovery` or `test discovery-matrix`. Build it in a Linux environment
matching the fixture's architecture and libc with `cargo xtask test rust --manifest-path
crates/newport-agent/Cargo.toml --locked --release --bench discovery_load --no-run`.
The static `discovery-build` command does not export this benchmark. A GNU-linked
benchmark needs a compatible glibc fixture; it cannot run directly on Alpine.
It creates bounded, temporary listener processes and measures the production
collector's latency, traversal counts and thread-local peak allocation. Its
helper subprocesses use the production CLI; protobuf round trips validate every
sample. Collector allocation measurements exclude other processes and assertions.
Each completed helper separately reports its main-thread peak, total and retained
allocation, elapsed time, operation and effective UID through a private benchmark
socket. Measurements include helper request decoding and reply encoding; startup
and reporting are outside the measured region. An unmatched helper start fails
the run. Root refresh and project scenarios require their corresponding reports.
These are requested Rust heap bytes, excluding allocator overhead, C allocations
and other threads. Per-process peaks are not a simultaneous process-tree peak
or resident-memory measurement. This instrumentation is only in the benchmark.
Use only an isolated Linux fixture; the runner also checks root-owned listeners
when elevation is available. It closes the fixture pipes and removes its copied
executable when finished.

The load cases include aggregate text exhaustion, excessive protobuf fields,
and project enrichment below and above the response budget. The fixtures use
real listener processes with bounded command lines and long executable/working
directory paths. Their own proc records provide independent input counts;
reports retain counts, not argument or path contents. Checks require preserved
listener identities, explicit limit diagnostics and complete protobuf round
trips. Each sample records final text bytes, protobuf fields and frame size.
Project-budget rejection must retain the valid base process details.

### Repeating Linux discovery qualification

Build the static Core agent and its discovery harnesses together, then run the
same executables in fresh distro containers. These commands require a macOS or
Linux host with Docker; the matrix requires a CLI and daemon supporting
`docker image inspect --platform` (API 1.49 or newer). Use fresh output
directories for each run:

```sh
cargo xtask test discovery-build --arch aarch64 --output "$PWD/target/discovery-build"
cargo xtask test discovery-matrix --arch aarch64 \
  --distro debian12 --distro alpine324 \
  --output "$PWD/target/discovery-matrix" \
  --agent "$PWD/target/discovery-build/artifacts/agent" \
  --unit-tests "$PWD/target/discovery-build/artifacts/unit-tests" \
  --helper-tests "$PWD/target/discovery-build/artifacts/helper-tests" \
  --monitor-tests "$PWD/target/discovery-build/artifacts/monitor-tests"
```

Use `--arch x86_64` for x86 executables. Cross-architecture Docker runs require
emulation. Omit `--distro` to run every preset serially: Debian 12/13, Ubuntu
22.04/24.04, AlmaLinux 9, Amazon Linux 2023, Arch, Alpine 3.24, Fedora 44,
openSUSE Leap 16 and Rocky Linux 9. Every distro runs with sudo allowed and then
denied. The runner uses unprivileged test users in containers limited to 2 GiB
memory, four CPUs and 512 processes. Arch disables pacman's download syscall
filtering during package setup because emulated seccomp may reject it; container
security and Newport test settings stay at their defaults.
The Arch preset requires x86_64; for aarch64, explicitly select other distros.

`discovery-build` exports `agent`, `unit-tests`, `helper-tests`, `monitor-tests`
and `systemd-tests`. It records exact source and executable SHA-256 hashes and
rejects sources changed during the build. Cargo's structured compiler output
selects the harness executables. The Docker context includes the protocol build
modules, and the exported executables must be static ELF binaries of the chosen
architecture. Full-agent qualification uses the same harnesses with a Full
executable built from the corresponding source; retain its own build provenance.
With `remote/` available, build it using `cargo xtask full-agent build --arch
x86_64 --output-dir "$PWD/target/discovery-full"`, then supply
`$PWD/target/discovery-full/x86_64/newport-agent` as `--agent` to the matrix.
Keep the checkout unchanged between the harness and Full-agent builds, and use
matching architectures. This Full-agent build is separate from the disk-monitored
`discovery-build` command; monitor available space during that build as well.

Each matrix receipt contains the tested binary hashes, selected platform image
ID, whole-reference ID, repository digests, OS/kernel, step results, logs and
cleanup outcome. Cached references are reused without pulling; if the requested
platform is absent, supply an available separate reference with `--image`.
Supplied executable hashes establish which bytes ran; they do not by themselves
establish source provenance. Keep the matching build receipt. For an exact image
rerun, select one distro and pass its recorded repository digest with `--image`.
Package repositories can change independently of a base-image digest; this is
a repeatable qualification workflow, not a frozen package mirror.

Both commands stop when host free space crosses `--reserve-gib` (default and
minimum 4 GiB). The matrix also monitors the container's filesystem. Output and
logs survive failure or interruption. A failed lane stops the matrix and removes
its owned container; preexisting images and unrelated containers are preserved.
Newly pulled images are removed after testing. Docker build caches remain under
Docker's management; interrupted pulls can leave partial cache data. Neither
command prunes shared caches or volumes. `NEWPORT_TEST_DOCKER_CONTEXT` selects
an explicit Docker context when required.

The shared CI workflow builds fresh static x86 fixtures and runs the Debian and
Alpine lanes, retaining receipts and logs on failure. The full distro matrix,
load benchmark, systemd VM and native desktop acceptance are separate runs.
Container tests share Docker's kernel and do not establish native kernel,
SELinux or desktop behavior.

With the private Remote apps source at `remote/`, its desktop discovery
acceptance guide (`remote/tests/README.md`) covers generated-protobuf checks
against an explicitly selected development app on macOS, Linux or Windows.
Those checks verify service
attribution and refresh generations without editing configuration or requiring
Pro. The guide also records the separate native interaction checklist.

The socketless NAT lifecycle has a separate real-service fixture:

```sh
cargo xtask test remote --filter remote_tests::containers::discovery::nat::
```

This creates an isolated rootful Podman engine in a privileged container, with
port reservation disabled. It exposes only the fixture's SSH port on host
loopback and does not mount the host engine socket. The test checks HTTP
reachability with no listening host socket, discovery through monitor and desktop
protobuf, and stop/restart/removal. Podman does not provide a stable engine ID;
the expected result is visible publications with partial coverage and retained
stale evidence, never authorization to clean up saved forwards. The fixture is
removed after success or a failed assertion. The normal remote-test command
prepares the Core agent resources before running; supplied prebuilt resources
must match the current source when using `NEWPORT_PREBUILT`.

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
| `cargo xtask package [--debug] [--skip-build] [--target <triple>] [--nsis] [--remote-apps]` | Stages the app and verifies it. macOS: `Newport.app` plus a zip. Windows: a zip, and with `--nsis` the installer. `--remote-apps` compiles Remote apps in on either desktop platform. Writes the third-party notices first, as `notices` does, into the app's `Licenses`. |
| `cargo xtask notices [--target <triple>] [--remote-apps] [--output <path>]` | Writes the third-party notices of a desktop package (default `target/THIRD-PARTY-NOTICES.txt`): the licenses of every crate in the app's, the grammar libraries' and the bundled agent's dependency graphs (the Full agent's with `--remote-apps`), rendered by [cargo-about](https://github.com/EmbarkStudios/cargo-about) with the root `about.toml` and `about.hbs`, then the changes Newport made to patched crates (`patches/<name>.NOTICE`, and `remote/patches/` with `--remote-apps`). Fails on a license `about.toml` doesn't accept, an unpublished crate that isn't Newport's, or a clarification whose text can't be fetched or changed. Installs the pinned cargo-about into `target/tools` first. |
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
The workflow publishes the verified packages and retains a `release-<tag>`
artifact. See [the release guide](releases.md).

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
tested), compile Remote apps in from the private Remote apps source at
`remote/`, which Git ignores here. A clone without it builds the free
edition.

For the build only, the streaming crates are added to newport-gpui with
`cargo add`, and GPUI is patched to this lockfile's revision on the command
line. A guard takes the crates out of `Cargo.toml` and `Cargo.lock` again when
the build ends, fails or is interrupted with Ctrl-C; a build that was killed
outright is cleaned up by the next `dev`, `build` or `test`. The checkout never
keeps a reference to the private crates.

For local GPUI development, `NEWPORT_GPUI_DIR` may name a GPUI snapshot
workspace. The build patches its members for both registry and Git consumers,
so the embedded presenter and Newport use the same types. This override is
transient and refused in CI; release CI uses the promoted, pinned snapshot.
Tests belonging to another workspace must run with that workspace's manifest
through `cargo xtask test rust --manifest-path ...`, rather than selecting a
dependency package from Newport's workspace.

A Remote apps build bundles the full agent (core and Remote apps) instead of
the core agent, from an export directory: `newport-agent-<arch>`, its
`newport-agent-<arch>.product.json`, provenance and an artifact manifest for
both architectures. `NEWPORT_AGENT_RESOURCE_DIR` names it. Without it, a local
build runs `cargo xtask agents --remote-apps`, which builds a development
export in Docker from this checkout and `remote/`, and keeps it in
`remote/target/unified-agent/desktop-resources`; with `NEWPORT_PREBUILT=1` it
uses that export as it is. Every export is checked against this commit, and in
CI against the private commit the release builds, before it is bundled.

`package --remote-apps` stages the full composition on macOS and Windows;
`release macos --remote-apps` selects it for the macOS release workflow.
With `--skip-build`, packaging checks the existing executable's composition
and rejects a core-only executable when Remote apps were requested.

Public CI checks the core edition without private dependencies. Remote apps
tests and native acceptance are [separate checks](testing.md#remote-apps-acceptance)
for maintainers with access to the private source.

A release builds the private source at its tag of the same name as the
release; a local build uses `remote/` as it is. A local production build
(`full-agent build --production`) needs this checkout at a release tag and
`remote/` at its tag of the same name. The release workflow builds the Full
agent for each architecture with `cargo xtask full-agent build --production`,
exports and signs it with `cargo xtask full-agent export --production` and
`cargo xtask release sign-agent`, and bundles that signed export into the macOS
app, where `release macos --remote-apps` checks the packaged app embeds exactly
that export before Developer ID signing and notarization.

## Not in xtask

- `tests/agent-runtime/private-directory.sh`, `tests/remote/start.sh` and
  `tests/remote/docker/start.sh` run inside containers.
- `tools/xtask/runner.sh` is the cargo runner shim: Cargo needs a runner that
  exists before anything is built.
