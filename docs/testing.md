# Testing

Newport uses Rust unit tests, real SSH integration tests, protocol contracts,
packaging checks, and desktop lifecycle tests. Each exercises a different
boundary. A passing headless suite does not prove GUI behavior, production
credential access, or a successful signed upgrade.

## Automated suites

Run commands from the repository root. Native builds use the pinned Rust and
Ghostty toolchains; `cargo xtask` supplies the Ghostty environment where needed.
Every command is described in [tools/xtask](../tools/xtask/README.md).

| Command | Coverage / requirements |
| --- | --- |
| `cargo xtask check rust` | Desktop crates: formatting, Clippy, and tests |
| `cargo xtask check agent` | Linux agent formatting, Clippy, and tests |
| `cargo xtask check xtask` | Build, packaging, release and harness tooling, with its unit tests |
| `cargo xtask test ssh` | Disposable SSH fixture, terminal, file, and RPC operations |
| `cargo xtask test remote` | Docker/Testcontainers with real OpenSSH and remote services |
| `cargo xtask test https` | Agent Git over HTTPS against a local `git http-backend` |
| `cargo xtask test git-contract` | Real agent Git responses through the app's validators |
| `cargo xtask test git-cli-contract` | Agent Git CLI responses through the app's validators |
| `cargo xtask test desktop` | Profile/preferences/metrics, resource staging, and the desktop app's lifecycle |
| `cargo xtask test desktop --headless` | Lifecycle checks without a GUI session |
| `cargo xtask test remote-apps` | newport-gpui's tests with Remote apps compiled in, from the private newport-remote checkout (CI's `remote-apps` job) |

`cargo xtask check` runs all three checks. Remote
checks use OrbStack by default; set `NEWPORT_TEST_DOCKER_CONTEXT=default` for the
standard Docker context. These fixtures mutate disposable test servers, not saved
production servers.

## Native behavior and rendering

```sh
cargo xtask grammars
cargo xtask env -- cargo test --locked -p newport-gpui --lib
```

Stage PDFium on Windows with `cargo xtask pdfium`. For lifecycle tests
alone, after staging resources:

```sh
cargo xtask env -- cargo test --locked -p newport-app --test standalone_lifecycle -- --ignored --nocapture
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

The opt-in macOS Keychain test signs a disposable provisioned bundle with the
[local signing settings](../tools/xtask/README.md#local-signing):

```sh
cargo xtask test keychain
```

It checks protected credential persistence and isolation using uniquely named
test accounts, then removes them. It does not establish access to a real user's
vault or certify an upgrade. Never log credentials or private key contents.

Packaging and release tooling tests are the `tools/xtask` unit tests. The opt-in, Windows-only `newport-release`
integration test `native_package_runtime` exercises an NSIS candidate named by
`NEWPORT_TEST_INSTALLER`; read its required environment before running it.

The `newport-client` integration tests `terminal_lifecycle`, `tunnel_edit`,
`history_replay`, `partial_index`, `worktree_lifecycle` and
`remote_refs_readonly` are opt-in (`--ignored`) and no `xtask` suite runs them
yet. They talk to a running Newport backend found through `NEWPORT_DATA_DIR`
and need a disposable loopback SSH server, for most a Docker one: set
`NEWPORT_TEST_SSH_PORT` (and, for the lifecycle test, `NEWPORT_TEST_LEASE_*`;
for `remote_refs_readonly`, `NEWPORT_TEST_READONLY_HOST`). Each test's
`#[ignore]` reason names what it needs. Confirm native Windows x64 and
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
