# Newport application

`newport-app` is the desktop entry point. It owns the backend runtime, profile
ownership, tray, window lifecycle, OS event registration, and updates. The
[GPUI library](../newport-gpui/README.md) owns presentation.

## Development

Use the pinned Rust toolchain. macOS builds require Xcode and its
Metal tools. Windows builds require the MSVC toolchain and Windows SDK; terminal
builds also use NASM for x64 or LLVM for ARM64.

Run from the repository root:

```sh
cargo xtask dev
```

This builds missing Linux agents in Docker, stages the grammar libraries when
their sources change (and PDFium on Windows), and runs the app with the
disposable profile `target/dev-profile`. Set `NEWPORT_DATA_DIR` to an absolute
path to use another isolated profile.

Debug builds require an explicit isolated profile. Release builds use the
default profile and preserve a recovery copy before opening it. Never run two
owners against the same profile. See [storage and recovery](../../docs/storage.md).

The development command supplies the pinned Ghostty environment. On Windows,
run `cargo xtask pdfium` before preview checks. When cross-compiling
on Windows ARM64, use the same `CARGO_BUILD_TARGET` for resource staging, build,
and packaging; running x64 under emulation is not native x64 acceptance.

Needle binaries live in `resources/agents`; `cargo xtask dev` builds them with
Docker only when they are missing. To rebuild them intentionally, use
`cargo xtask agents`. This replaces local agent resources; do not rebuild them
merely to run the desktop with verified resources.

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
cargo xtask package
```

This produces a development package. On Windows, add `--nsis` to build the
installer; use `--target` to select the matching architecture explicitly.
Production packages use [.github/workflows/release.yml](../../.github/workflows/release.yml):
macOS Apple Silicon and Windows ARM64/x64, signed agent/update payloads, macOS
Developer ID signing and notarization, a combined update manifest, checksums,
and automatic R2 publication, with a retained `release-<tag>` workflow artifact.
Website deployment is managed separately.
See [release publishing](../../docs/releases.md). Packaging success does not certify upgrades.

## Automatic updates

Supported standalone release builds check after startup and periodically,
download a newer signed package, and offer **Restart to update**. Installation
waits for the user to restart. Debug, isolated-profile, and App Store builds do
not use this updater.

The updater reads `https://newport.app/updates/latest.json` and downloads
versioned packages from the website's R2 storage. It verifies packages against
the existing public key. macOS replacement
uses a journal, health checks, and rollback; Windows drains the app before
launching the installer. Preserve the original signing key and the existing
credential/profile identities across releases.

The platform crate includes `update_check`, `update_transaction_check`, and
`update_startup_check` examples for disposable update checks. The transaction
example requires `NEWPORT_UPDATE_TEST_APP` pointing to a disposable older app;
the startup example uses `NEWPORT_UPDATE_TEST_EXECUTABLE` and an isolated
`NEWPORT_DATA_DIR`. Run with `--features updates`. Neither substitutes for the
[signed upgrade acceptance procedure](../../docs/testing.md#upgrade-acceptance).

## Tooling entry points

Development, checks, packaging and release run through `cargo xtask`; see
[tools/xtask](../../tools/xtask/README.md) for every command.

| Task | Entry point |
| --- | --- |
| Run / build the desktop | `cargo xtask dev` / `cargo xtask build` |
| Format / validate the workspace | `cargo fmt --all` / `cargo xtask check` |
| Build Linux agents | `cargo xtask agents` |
| Desktop / SSH / container tests | `cargo xtask test desktop` / `cargo xtask test ssh` / `cargo xtask test remote` |
| Grammar / PDFium resources | `cargo xtask grammars` / `cargo xtask pdfium` |
| Development packages | `cargo xtask package` |
| macOS distribution | `cargo xtask release macos` |
| Windows update payload | `cargo xtask release windows-update` |
| Needle release identity | `cargo xtask release sign-needle` |
| Verify and combine updater manifests | `cargo xtask release merge` |

Run the app with `cargo xtask dev`, which prepares the build environment (a
plain `cargo run` stops at the Ghostty build script). On macOS it then goes
through the committed Cargo runner, `tools/xtask/runner.sh`, which runs
`cargo xtask dev-run`. It bundles and signs
a provisioned development app with the settings described in
[local signing](../../tools/xtask/README.md#local-signing); it is not a
production signing shortcut. Protected-Keychain acceptance uses
`cargo xtask test keychain`. The runner launches the GUI through LaunchServices
so 1Password attributes SSH approval to Newport. Do not launch
`Newport.app/Contents/MacOS/newport` directly from an IDE or terminal for
interactive SSH testing: macOS can attribute those requests to the parent
application and 1Password suppresses its background prompts. The runner
preserves `NEWPORT_DATA_DIR` (including relative paths), `NEWPORT_ACCOUNT_URL`,
`SSH_AUTH_SOCK`, and Rust logging settings. GUI output goes to
`newport-dev.log` beside the bundle; Cargo waits until the app quits. Headless
`--check-*` probes and `--build-info` keep direct stdout and exit status.
On an interactive Windows desktop, run
`./scripts/test-windows-activation.ps1 -BinaryPath C:\path\to\newport.exe`
to check close-to-tray and second-instance activation using a disposable profile.
