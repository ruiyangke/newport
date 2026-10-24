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

On Windows, check HTTPS certificate validation against both the production feed
and the legacy GitHub redirect path before release. The loopback updater tests
do not exercise Schannel or the Windows trust store. This opt-in check downloads
only manifests; it does not install anything:

```sh
cargo test --locked -p newport-platform --features updates --lib updates::tests::windows_trusts_production_and_legacy_update_feeds -- --exact --ignored
```

### Released Porthop agent regression

`tests/agent-runtime/porthop-upgrade.py` exercises a real Porthop 0.2.3 agent
extracted from its published desktop archive. It verifies the archive checksum,
uses that release's upload installer, starts the old runtime, then stages,
negotiates with, activates and reinstalls the current agent. It covers Bash,
Zsh, Fish, installation without integrations, and existing native commands.
Custom, absolute, chained and dangling Porthop aliases must disappear; unrelated
files, links and system clipboard commands must survive. The old process and its
clipboard cache must also be removed.

Use the `newport-test-remote:local` image built by the remote test suite and a
fresh Linux `newport-agent` matching its CPU. Keep test binaries in a temporary
directory, separate from signed release resources. From the repository root:

```sh
upgrade_dir=$(mktemp -d)
curl --fail --location \
  https://github.com/ruiyangke/newport/releases/download/v0.2.3/Porthop-0.2.3-macos-arm64.app.tar.gz \
  --output "$upgrade_dir/Porthop-0.2.3-macos-arm64.app.tar.gz"
git show v0.2.3:src-tauri/src/agent-install.sh > "$upgrade_dir/porthop-install.sh"
# Copy the freshly built Linux binary to "$upgrade_dir/newport-agent".
docker run --rm --user fixture --network none --entrypoint python3 \
  -v "$PWD:/workspace:ro" -v "$upgrade_dir:/upgrade:ro" \
  newport-test-remote:local \
  /workspace/tests/agent-runtime/porthop-upgrade.py /upgrade \
  > "$upgrade_dir/upgrade.log" 2>&1
cat "$upgrade_dir/upgrade.log"
```

The macOS archive contains both Linux architectures; no desktop executable runs.
All installation homes are temporary directories inside the container, which
runs as the unprivileged fixture account without network access. The log records
binary hashes and command links before and after each scenario. This exercises
the production installer scripts and runtime protocol directly, without the
desktop UI or SSH transport.

### Desktop release upgrades

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
