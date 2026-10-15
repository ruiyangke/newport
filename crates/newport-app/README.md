# Standalone Newport application

This crate composes the native GPUI interface with `newport-backend` and the OS
adapters in `newport-platform`. Neither the UI nor backend crate owns the other's
runtime. The application uses the existing private local RPC contract and runs
without a Tauri host.

## Development

Debug builds require an explicit isolated profile. Release builds resolve the
existing profile and preserve it before migration; production cutover and
automatic updates still require signed cross-platform acceptance. Quit any existing native
owner before switching binaries for the same profile. On macOS, keep the
repository's signed Cargo runner configured so the existing protected Keychain
entry remains accessible.

```sh
node scripts/build-grammars.mjs
NEWPORT_DATA_DIR=/absolute/path/to/test-profile cargo run --locked -p newport-app
```

Append `-- --check-backend` to start the real backend, read its snapshot and shut
down without opening a window. The check refuses a profile already in use.
A normal second launch activates the existing owner's window and exits instead
of starting another backend. Owners without the activation endpoint return an
explicit error; they are never replaced or bypassed. No
plaintext vault-key override is needed for an existing signed development profile.

For repeatable persistence and process-level acceptance, run `npm run test:native`.
It stages grammars (and PDFium on Windows), runs native profile/preferences/metrics
tests, then invokes the opt-in standalone lifecycle suite. Add `-- --headless` to
skip the macOS GUI case. To run just lifecycle checks after staging dependencies,
use `cargo test --locked -p newport-app --test standalone_lifecycle -- --ignored --nocapture`.
The suite owns disposable profiles and tests the Cargo-built standalone binary;
`NEWPORT_TEST_APP` can select an absolute native package executable instead.
Headless startup/ownership checks run on all release targets. The macOS-only GUI
case requires an interactive desktop and covers activation and graceful shutdown.
See [test coverage](../../docs/testing.md) for limits and the pinned released
desktop baseline. The source Tauri host and its harness have been removed.

On Windows, use a Visual Studio Developer Command Prompt with the matching MSVC
target installed. The x64 build also requires [NASM](https://www.nasm.us/) on
`PATH` for AWS-LC's assembly; LLVM and the Windows SDK are required by the native
dependencies. On an ARM64 Windows host, select `vcvarsall.bat arm64_x64` when
testing the x64 build and set `CARGO_BUILD_TARGET=x86_64-pc-windows-msvc` for every
build, resource-staging and packaging command. Use `aarch64-pc-windows-msvc` for
the ARM64 build. A successful ARM64 run does not verify the x64 binary.

`--check-update-health` additionally validates the bundled grammars and PDF renderer, reads the
profile through real RPC, and drains backend services before returning a small
JSON receipt with the native executable's version. It never opens a window or
activates another instance. An unreadable/locked profile, malformed snapshot,
startup error or shutdown failure returns failure without a success receipt.
This is the headless prerequisite for update readiness; it does not certify GUI
startup or remote connections.

`--autostart` starts the UI hidden, with the backend and tray running. A second
login launch exits without raising the existing window. A tray initialization
failure reveals the window so the application remains accessible. Registration
uses the shared platform adapter: the existing `Newport.plist` LaunchAgent on
macOS and the `Newport` per-user Run entry on Windows. Debug and explicit-profile
builds cannot change the installed application's login registration.

Window close hides the interface and preserves background work. Explicit Quit,
Ctrl-C or (on Unix) SIGTERM requests shutdown; the application waits for backend
workers and sessions to drain. Unexpected backend termination requests UI exit
and returns an error. Clipboard requests are dispatched on the GPUI main thread.
The same thread installs native sleep/wake and network observers. They signal
the backend's existing recovery policy; installation failures retain timed
recovery and are reported in diagnostics.

For SSH diagnosis against a saved server, run:

```sh
NEWPORT_DATA_DIR=/absolute/path/to/test-profile cargo run --locked \
  -p newport-client --example connection_read_check -- saved-host
```

This reports authentication mode, agent availability and an active consent wait
without printing identity comments, fingerprints or profile contents. Append
`--authenticate` to run Newport's read-only `printf ok` connection check; signing
may require approval in the SSH agent. The shell also shows an active signing
wait in its existing connection-status label.

The native tray offers Open Newport, Settings and Quit, and shows connected
tunnel count (or an error marker when none are connected). Status reads run off
the GUI thread; an unavailable snapshot clears the count and reports unavailable
status. The icon and its menu belong to the app's main-thread lifetime.

Launch-at-login and application updates are explicitly unavailable in this
migration build. Windows tray and activation acceptance, physical
sleep/network recovery acceptance, complete Windows desktop acceptance, and
signed distribution/upgrade testing remain required before production cutover. See the
[retirement plan](../../docs/plans/2026-10-11-tauri-retirement.md).

The native host owns Settings update status/check commands and their worker
lifetime. Checks run off the UI and async executor threads, with at most one
active download. A verified package remains stable while restart confirmation
is open; disconnecting a Settings request does not lose that package or start a
duplicate download. Explicit failures remain retryable. Shutdown cancels checks,
discards retained packages and ignores late progress/results. After backend
services have drained, Quit does not wait out a stalled HTTPS download deadline.
The original background cadence (30 seconds after startup, then every six hours)
is preserved for eligible builds. The release-acceptance gate still disables
checks and installation in migration builds. macOS restart/install coordination
and normal-startup recovery/acceptance are implemented; signed GUI upgrade and
profile acceptance, plus Windows installer/runtime acceptance, remain required
before enabling production use.

The native updater's discovery/download/verification layer can be exercised
independently without installing anything:

```sh
cargo run --locked -p newport-platform --features updates --example update_check
```

It uses the shared production endpoint and public key, checks for a release newer
than `resources/app.json`, and verifies any downloaded package and its signed
version. The platform crate also stages validated macOS bundles and Windows
installers without modifying the installed application. On macOS, a separate
transaction API provides atomic replacement, journal discovery and restoration,
with the old bundle retained until the relaunched host confirms health. Pending
recovery prevents another update from beginning. These operations, restart
coordination and Settings are now connected for macOS behind the release gate.
The install command requires the confirmed version, stages in a worker and hands
ownership to the app once. The host records recovery before draining services;
replacement runs only after successful shutdown. A failed replacement, headless
startup check or process launch restores and restarts the previous app through
the same journal. Successful relaunch retains the backup until the root window
initializes and the running backend confirms a readable profile. Automatic
startup recovery holds the installation lock while reserving profile ownership,
and resolves mismatched versions before starting backend services. Quit cancels
pending acceptance or waits for cleanup already begun. UI initialization does
not prove rendered-frame or full workflow acceptance. Cancelling staging during
Quit waits for its temporary-file cleanup before runtime teardown.

The Windows host now hands the authenticated raw NSIS package to the OS only
after service drain. It preserves the shipping passive-update/restart flags,
revalidates and locks the staged file and directory through process creation,
and restarts the existing app if launch fails or is cancelled. Successful handoff
retains the private installer directory because the installer still needs it
after the app exits. This is not installation-success evidence. Native NSIS
packaging, retained-installer cleanup, UAC/relaunch, interrupted-install recovery
and real Windows upgrade testing remain open; the release gate stays closed.

To exercise real signature verification, replacement and restoration using a
disposable copy of an older signed app:

```sh
NEWPORT_UPDATE_TEST_APP=/absolute/path/to/older/Newport.app \
  cargo run --locked -p newport-platform --features updates \
  --example update_transaction_check
```

The example requires a newer release at the production endpoint. It modifies
only its temporary copy, discovers recovery after interruptions, restores the
copy and removes the backup. It never launches either version or opens a profile.
It does not establish production upgrade or relaunch acceptance.

The parent-side startup probe can be exercised against a trusted native
executable with an isolated profile that is not already in use:

```sh
NEWPORT_DATA_DIR=/absolute/path/to/test-profile \
NEWPORT_UPDATE_TEST_EXECUTABLE=/absolute/path/to/Newport.app/Contents/MacOS/newport \
  cargo run --locked -p newport-platform --features updates \
  --example update_startup_check
```

It imposes a deadline and output limit, requires both a successful process exit
and the exact expected version's receipt, and kills/reaps failed probes. It does
not accept an update transaction or remove its backup. The macOS coordinator
validates the candidate first, drains the previous backend and retains recovery
through normal GUI startup.

## Bundled language grammars

Syntax parsers and their matching highlight queries ship in `newport-grammars`,
a C-ABI shared library. The Tree-sitter runtime remains linked into Newport.
The app keeps the library loaded for the process lifetime and validates grammar
ABI versions and queries before registering languages and aliases. It loads
only from the installation directory, never from the working directory or PATH.

Build/stage grammars before running the native app or its tests:

```sh
node scripts/build-grammars.mjs
cargo test --locked -p newport-gpui --lib
```

For release builds and a development ZIP (not a production-signed release):

```sh
node scripts/package-native.mjs
```

On Windows, add `--nsis` to build an unsigned native installer as well as the
archive. `--debug --skip-build --nsis` packages an existing debug build for CI.
The host embeds Newport's shared icon as resource group `1` (used by GPUI) and
the product/version information shown by Windows. Missing or invalid resource
compilation fails the build. GPUI continues supplying its own DPI/common-controls
manifest, avoiding duplicate application manifests. Windows CI checks the built
executable's version resources against `resources/app.json` before packaging.
An explicit `--target` or `CARGO_BUILD_TARGET` selects its Cargo output directory.
Packaging checks `--build-info` against the shared product metadata and requested
architecture, then checks bundled grammars. Neither check opens a profile.

The installer compiler can also run on macOS/Linux against an already staged
Windows application (requires `makensis`):

```sh
node scripts/package-windows-native.mjs --app /path/to/Newport \
  --arch x86_64 --output /path/to/Newport-x86_64-setup.exe
```

It validates the payload file list and PE architecture, snapshots the inputs,
and publishes the output only after compilation succeeds. The NSIS candidate
uses the shipping current-user install identity and preserves profile files,
including when they share the installation directory. Only known application
files participate in replacement and uninstall. `/P /UPDATE /R` is supported;
legacy `/ARGS` forwarding and optional deletion of user data are not implemented.
Compiler tests do not prove Windows install/recovery behavior. Native runtime,
signed upgrades and production profile acceptance remain required. CI and release
workflows now build native artifacts; changing the build path does not close those gates.

Grammar metadata is registered at startup without opening native libraries. Each
language library loads on its first parser request, including embedded languages,
and stays loaded for the process lifetime. Missing or incompatible grammars fall
back to plain text. `--check-grammars` explicitly loads and validates all languages
for packaging.

The macOS package keeps the per-language libraries and `catalog.json` under
`Newport.app/Contents/Resources/grammars/`; Windows keeps `grammars/` beside
`newport.exe`. The macOS development runner signs every library before signing the
app. Production packaging must likewise sign native libraries before the bundle
and notarize the finished app. The packaging script's macOS signature is ad hoc;
it does not replace provisioning or release acceptance.

For a local Developer ID distribution candidate, use
`node scripts/release-native-macos.mjs`. It builds the ARM64 release application,
signs each language's grammar library before the host, submits to notarization,
requires an `Accepted` result, staples the ticket and prepares updater artifacts. It never
publishes or installs the candidate. `--skip-build` reuses release binaries, but
still validates their version, architecture and disabled debug assertions.

Set `APPLE_SIGNING_IDENTITY` to a Developer ID Application identity,
`NEWPORT_PROVISIONING_PROFILE` and `NEWPORT_SIGNING_ENTITLEMENTS` to the matching
files, and `NEWPORT_NOTARY_PROFILE` to an existing notarytool Keychain profile.
The native updater signing environment is described in
[newport-release](../newport-release/README.md). The credential profile avoids
passing an Apple password on the notarization command line. Output is under
`target/aarch64-apple-darwin/release/native-package/distribution/update`, or the
equivalent configured Cargo target directory. This path has not yet passed real
notarization and upgrade acceptance; production startup remains gated.

Both the development runner and native packaging derive Info.plist identity and
version from the executable's validated `--build-info` receipt. They no longer
write a hardcoded development version. The receipt also lets distribution
packaging reject stale, wrong-architecture or debug binaries.

Use `newport --check-grammars` to validate a staged installation without opening
any profile or window. Missing/incompatible libraries produce an explicit error.
When changing grammar versions, update the shared Cargo.lock and bundled queries
together, then run grammar and highlighting tests. The query/alias definitions
were adapted from GPUI Kit bfc64ce6; their Apache-2.0 license is included.

## PDF previews

PDF rasterization and selectable text belong to `newport-platform`; GPUI only
adapts the returned pixels and text geometry. macOS uses Core Graphics/PDFKit.
Windows uses a pinned PDFium build without V8 or XFA. Stage its DLL before
running the Windows app or PDF tests:

```sh
node scripts/stage-pdfium.mjs
cargo test --locked -p newport-platform --features pdf pdf::tests
```

The staging command honors `--target`, `CARGO_BUILD_TARGET`, and `--release`.
It verifies the archive, DLL, and combined license notices against
`resources/pdfium.json`. Packaging includes the DLL and all upstream license
notices, and runs `--check-pdf` to render a fixture and verify selectable text.
The installer runs the same check before committing an update. The application
never downloads a renderer at runtime or searches the current directory/PATH.

Both renderers enforce page/raster/text limits, handle crop and rotation, retain
Unicode byte ranges for selection, and respect copy restrictions. Password-only
documents report an error without prompting. `--check-pdf` opens no profile.
