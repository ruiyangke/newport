# Testing

Newport uses Rust unit tests, real SSH integration tests, protocol contracts,
packaging checks, and native desktop lifecycle tests. Each exercises a different
boundary. A passing headless suite does not prove GUI behavior, production
credential access, or a successful signed upgrade.

## Automated suites

Run commands from the repository root. Native builds use the pinned Rust and
Ghostty toolchains; the npm commands supply the Ghostty wrapper where needed.

| Command | Coverage / requirements |
| --- | --- |
| `npm run check:rust` | Desktop crates: formatting, Clippy, and tests |
| `npm run check:agent` | Linux agent formatting, Clippy, and tests |
| `npm run check:harness` | Native/remote harness checks |
| `npm run test:tooling` | Release scripts, resources, installers, and manifests |
| `npm run check:contracts` | Independent Git protocol oracle; first run `npm ci --prefix tools/git-contract` |
| `npm run test:ssh` | Disposable SSH fixture, terminal, file, and RPC operations |
| `npm run test:remote` | Docker/Testcontainers with real OpenSSH and remote services |
| `npm run test:git-contract` | Git protocol against the independent reference |
| `npm run test:git-cli-contract` | Git behavior against the reference CLI |
| `npm run test:native` | Profile/preferences/metrics, resource staging, and native lifecycle |
| `npm run test:native -- --headless` | Lifecycle checks without a GUI session |

`npm run check` combines static checks and unit/tooling/contract suites. Remote
checks use OrbStack by default; set `NEWPORT_TEST_DOCKER_CONTEXT=default` for the
standard Docker context. These fixtures mutate disposable test servers, not saved
production servers.

## Native behavior and rendering

```sh
node scripts/build-grammars.mjs
node scripts/with-ghostty.mjs cargo test --locked -p newport-gpui --lib
```

Stage PDFium on Windows with `node scripts/stage-pdfium.mjs`. For lifecycle tests
alone, after staging resources:

```sh
node scripts/with-ghostty.mjs cargo test --locked -p newport-app --test standalone_lifecycle -- --ignored --nocapture
```

Lifecycle fixtures cover encrypted profile retention, ownership exclusion,
repeated startup, corrupt-vault refusal, and upgrade backups. `NEWPORT_TEST_APP`
can select an absolute path to a matching native package. GUI cases need an interactive desktop; wait for rendered state
between steps, not just process startup. Record keyboard/focus, scrolling,
clipboard, terminal, reconnect, tray, and shutdown results on each platform.

See the [UI guide](../crates/newport-gpui/README.md) for rendering benchmarks and
manual input probes. Keep benchmark evidence outside source control and record
the commit, build profile, platform, and fixture size with each result.

## Signing and installation checks

The opt-in macOS Keychain test uses a disposable signed/provisioned bundle:

```sh
APPLE_SIGNING_IDENTITY='Developer ID Application: Your Name (TEAMID)' \
NEWPORT_PROVISIONING_PROFILE=/absolute/path/Newport.provisionprofile \
NEWPORT_SIGNING_ENTITLEMENTS=/absolute/path/Newport.entitlements \
node scripts/test-macos-keychain.mjs
```

It checks protected credential persistence and isolation using uniquely named
test accounts, then removes them. It does not establish access to a real user's
vault or certify an upgrade. Never log credentials or private key contents.

Packaging tests live in `scripts/ci/`. The opt-in `newport-release` integration
test `native_package_runtime` exercises staged packages on the matching host;
read its required environment before running it. Confirm native Windows x64 and
ARM64 separately. Emulation is useful evidence but not a replacement for native
installer acceptance.

For a release, record the exact tagged source and workflow run. Verify all three
platform packages, updater signatures, `latest.json`, and `SHA256SUMS`; check
macOS signing, notarization, and stapling. Passing CI alone does not authorize
publishing or moving a version tag.

## Upgrade acceptance

For production-path credential and upgrade acceptance, use a disposable OS account
or VM snapshot. Install the previous release, create fixture servers, passwords,
bookmarks, preferences and metrics through its UI, then quit it normally. Record
profile and credential-store evidence without printing keys or passwords. Install
a production-signed candidate using its real installer/update route and
verify credential access, retained data, ordinary/default startup, UI activation,
remote operations, restart, login registration and rollback/recovery. Test macOS
ARM64 and both Windows architectures; x64 emulation is not native x64 hardware
acceptance. Restore the disposable environment afterward.

Production builds support default-profile startup and automatic updates. Do not
treat an unsigned/isolated build as the signed upgrade test. A passing fixture
suite or changed release workflow does not certify this acceptance. Record the actual candidate version, digest, platform, route and
observed results when each check is executed.

macOS replacement/recovery can also be exercised on a disposable app copy
with `newport-platform`'s `update_transaction_check` example; see
[the application guide](../crates/newport-app/README.md). It requires a suitable
newer signed release and does not launch either app or establish a profile/UI
upgrade.
