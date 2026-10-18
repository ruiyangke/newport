# Newport application

`newport-app` is the desktop entry point. It owns the backend runtime, profile
ownership, tray, window lifecycle, OS event registration, and updates. The
[GPUI library](../newport-gpui/README.md) owns presentation.

## Development

Use the pinned Rust toolchain and Node.js. macOS builds require Xcode and its
Metal tools. Windows builds require the MSVC toolchain and Windows SDK; terminal
builds also use NASM for x64 or LLVM for ARM64.

Run from the repository root:

```sh
node scripts/build-grammars.mjs
NEWPORT_DATA_DIR=/absolute/path/to/test-profile npm run dev
```

Debug builds require an explicit isolated profile. Release builds resolve the
existing profile and preserve recovery copies before migration. Never run two
owners against the same profile. See [storage and recovery](../../docs/storage.md).

The development command supplies the pinned Ghostty environment. On Windows,
run `node scripts/stage-pdfium.mjs` before preview checks. When cross-compiling
on Windows ARM64, use the same `CARGO_BUILD_TARGET` for resource staging, build,
and packaging; running x64 under emulation is not native x64 acceptance.

Needle binaries must already be present in `resources/agents`. To rebuild them
intentionally, use `npm run build:agent` with Docker. This replaces local agent
resources; do not rebuild them merely to run the desktop with verified resources.

## Lifecycle and diagnostics

Closing the window hides it while background work continues. Explicit Quit and
termination signals drain backend workers and sessions. `--autostart` starts
hidden with the tray; isolated/debug builds cannot change the installed app's
login registration. Sleep/wake and network callbacks notify backend recovery.

The executable provides these checks:

| Argument | Checks |
| --- | --- |
| `--build-info` | Version, target, and build identity without opening a profile |
| `--check-needle` | Bundled agent resources |
| `--check-grammars` | Bundled grammar libraries |
| `--check-pdf` | Native PDF renderer |
| `--check-backend` | Backend startup and profile access |
| `--check-update-health` | Resources, real profile RPC, and graceful backend shutdown |

Use an explicit disposable `NEWPORT_DATA_DIR` for development health checks.
A health receipt establishes headless startup, not GUI or remote-operation
acceptance. Protected macOS Keychain checks require the matching signing and
provisioning setup; see [testing](../../docs/testing.md).

## Resources and packaging

Each language grammar is a separate shared library, loaded on demand and kept
alive while its parser is used. PDF previews use PDFKit on macOS and staged
PDFium on Windows. Needle binaries are compressed resources verified before
remote installation.

```sh
node scripts/package-native.mjs
```

This produces a development package. On Windows, add `--nsis` to build the
installer; use `--target` to select the matching architecture explicitly.
Production packages use [.github/workflows/release.yml](../../.github/workflows/release.yml):
macOS Apple Silicon and Windows ARM64/x64, signed agent/update payloads, macOS
Developer ID signing and notarization, a combined update manifest, checksums,
and an unpublished draft release. Packaging success does not certify upgrades.

## Automatic updates

Supported standalone release builds check after startup and periodically,
download a newer signed package, and offer **Restart to update**. Installation
waits for the user to restart. Debug, isolated-profile, and App Store builds do
not use this updater.

The updater verifies packages against the existing public key. macOS replacement
uses a journal, health checks, and rollback; Windows drains the app before
launching the installer. Preserve the original signing key and the existing
credential/profile identities across releases.

The platform crate includes `update_check`, `update_transaction_check`, and
`update_startup_check` examples for disposable update checks. The transaction
example requires `NEWPORT_UPDATE_TEST_APP` pointing to a disposable older app;
the startup example uses `NEWPORT_UPDATE_TEST_EXECUTABLE` and an isolated
`NEWPORT_DATA_DIR`. Run with `--features updates`. Neither substitutes for the
[signed upgrade acceptance procedure](../../docs/testing.md#released-desktop-baseline).

## Script entry points

Use root npm commands for development and checks. The files under `scripts/ci/`
contain CI setup, artifact validation, and tooling tests; they are not additional
application launch paths.

| Task | Entry point |
| --- | --- |
| Run / build the desktop | `npm run dev` / `npm run build` |
| Format / validate the workspace | `npm run format` / `npm run check` |
| Build Linux agents | `npm run build:agent` (delegates to the Rust harness) |
| Native / SSH / container tests | `npm test` / `npm run test:ssh` / `npm run test:remote` |
| Grammar / PDFium resources | `scripts/build-grammars.mjs` / `scripts/stage-pdfium.mjs` |
| Development packages | `scripts/package-native.mjs` |
| macOS distribution | `scripts/release-native-macos.mjs` |
| Windows installer / update payload | `scripts/package-windows-native.mjs` / `scripts/package-windows-update.mjs` |
| Needle release identity | `scripts/sign-needle-release.mjs` |
| Verify and combine updater manifests | `scripts/merge-update-manifests.mjs` |

Run JavaScript entry points with Node. `with-ghostty.mjs` prepares the pinned
terminal toolchain; its source/stdlib helpers keep patches isolated. Shared
signing, notarization, build-info, and grammar modules serve the packaging steps.
They should stay separate from OS-specific installers and release credentials.

`scripts/run-signed-dev.sh` is an optional local Cargo runner for a provisioned
macOS development app. It is not a production signing shortcut. Protected-Keychain
acceptance uses `scripts/test-macos-keychain.mjs` with explicit test configuration.
The runner launches the GUI through LaunchServices so 1Password attributes SSH
approval to Newport. Do not launch `Newport.app/Contents/MacOS/newport` directly
from an IDE or terminal for interactive SSH testing: macOS can attribute those
requests to the parent application and 1Password suppresses its background prompts.
The runner preserves `NEWPORT_DATA_DIR` (including relative paths), the legacy
`PORTHOP_DATA_DIR`, `SSH_AUTH_SOCK`, and Rust logging settings. GUI output goes to
`newport-dev.log` beside the bundle; Cargo waits until the app quits. Headless
`--check-*` probes and `--build-info` keep direct stdout and exit status.
On an interactive Windows desktop, run
`./scripts/test-windows-activation.ps1 -BinaryPath C:\path\to\newport.exe`
to check close-to-tray and second-instance activation using a disposable profile.
Offline asset/terminal generators remain in
[the UI scripts package](../newport-gpui/scripts/README.md); the desktop does not
install those npm dependencies.
