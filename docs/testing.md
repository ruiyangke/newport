# Test coverage

Newport uses unit tests, browser tests, real SSH integration tests and native desktop checks. These layers are complementary: browser tests mock Tauri, while the remote suite calls the production Rust backend against disposable Linux servers. Neither alone is a full native UI end-to-end test.

## Run the suites

```sh
npm run check          # frontend, Rust, agent and manifest checks; no Docker required
npm run test:remote    # Testcontainers + real OpenSSH, defaults to OrbStack
```

The agent CI uses the locked Nix environment on Linux and macOS. To run the same formatting, lint and test checks locally:

```sh
nix develop --command npm run check:agent
```

Commit both `flake.nix` and `flake.lock`. The lock pins the package collection and Rust overlay; the shell selects Rust 1.98.0, Node 22 and the clipboard clients and shells used by tests. Update the lock with `nix flake update`, then rerun the checks on Linux and macOS. Nix does not replace the Docker engine, Xcode/signing tools, or Windows build tools. The Windows desktop and release packaging jobs still use native toolchains.

The backend runner currently requires a macOS host (the desktop backend does not support Linux). The fixture itself is Linux. The remote suite requires Rust, Node, Python 3, OpenSSH tools and a running local Docker-compatible engine. It builds the Linux agents and the fixture image. To use another local Docker context:

```sh
NEWPORT_TEST_DOCKER_CONTEXT=default npm run test:remote
```

The Python entry point only builds prerequisites and resolves the Docker context. The Rust Testcontainers runner owns container readiness, dynamic localhost port mapping, file provisioning and removal. It pins the server host key through the container API, uses a temporary key and private SSH agent, and runs backend tests in a child process so test credentials never affect other tests or saved profiles. The suite is serial because its tests share a remote account and mutate the agent installation. It captures server logs on test failure and imposes a ten-minute test timeout.

Disk-full tests use a disposable 4 MiB tmpfs; they never fill the host filesystem. SSH fault tests kill only the disposable account’s SSH workers, preserving the server listener.

The fixture installs Bash, Zsh and Fish; missing shells fail the suite rather than silently skipping tests. It does not mount your home, SSH agent, application data or Docker socket. Container cleanup is automatic on ordinary completion/failure; force-killing the runner can still require manual Docker cleanup.

## Feature matrix

“Container” means a real Linux/OpenSSH path exercised by `npm run test:remote`. “Partial” deliberately does not mean full feature coverage. Test identifiers below live in `src-tauri/src/remote_tests.rs` and its `recovery.rs`, `boundaries.rs` and `arboard.rs` submodules.

| Feature | Container coverage | Other coverage / remaining gaps |
|---|---|---|
| Profiles and SSH authentication | Partial: `authentication_exec_sftp_and_metrics`, `changed_host_key_is_rejected_and_restoration_recovers` | Profile UI/validation tests; OS credential store, password authentication and 1Password need native checks |
| SSH execution | Unicode, large stdin, nonzero exit/stderr; actual SSH worker termination | Packet loss and host sleep/wake remain |
| Persistent tunnels | HTTP round trip, automatic reconnect after real SSH disconnect, cancellation during backoff, listener cleanup, port-range bind rollback | Full container restart and host sleep/wake remain |
| Discovery and destination health | Real SSH/HTTP listener discovery; reachable vs unavailable destination without marking SSH disconnected | Container/process attribution across different permissions remains |
| Overview metrics | Basic real Linux collection | Collector fixtures; broad host configurations remain |
| Metrics history, retention and cache | Not covered by remote suite | Database/history/UI tests; long-running sampler-to-database journey remains |
| Services/logs | Missing systemd returns a real command error | Successful systemd listing/logs need a systemd-capable fixture |
| Docker/Compose operations/logs | Missing Docker returns a real command error | Successful operations/logs need an isolated Docker daemon fixture |
| Interactive terminal | `interactive_shell_resize_and_exit` | Mocked xterm UI; full native terminal journey remains |
| Files | Binary round trip, Unicode names, overwrite/permission protection, cancelled upload cleanup and retry, cancelled download preserving existing files; source mutations reject partial publication; directory/symlink listing and preview type/size limits | Native dialogs and a network interruption during transfer remain |
| Agent installation/reinstallation | Deployment, idempotent install/reinstall and hash consistency; bad upload checksum preserves installed binary and removes staging file; managed older-version stub is replaced | Stub replacement is not an actual old-release compatibility test; upgrade of a running older binary remains |
| Clipboard on demand | Compressed transfer, revision/cache changes, corrupt-response retry; a new copy invalidates an in-flight read and rejects late old bytes; clearing removes cached data; concurrent readers share one request | Desktop clipboard provider is simulated; actual OS image freshness across SSH remains |
| X11/Wayland clipboard | Native xclip/wl-paste plus unmodified arboard: Unicode, repeated reads, PNG pixel hashes, large compressed images, malformed/empty formats, superseded image rejection and agent restart | Actual OS clipboard source and automatic reconnection of an existing X11 client are not covered |
| Browser forwarding | URL/ack, unsafe URL rejection, browser-only isolation, mismatched ack ignored; real HTTP callback, occupied-port rejection, bounded leases and rollback; closed callback SSH session does not reconnect | Actual browser launch, hidden callback detection and five-minute expiry remain |
| Shell setup | `shell_setup_is_idempotent_and_respects_protected_files`: Bash/Zsh/Fish, repeat install, PATH uniqueness, missing/failing agent, read-only files and symlinks | Additional marker/ownership unit tests; filesystem ACL behavior remains |
| Recovery | Persistent tunnel automatic retry/cancellation; killed agent can restart without stale clipboard; actual ENOSPC yields retryable error and successful restart after freeing space; malformed clipboard reply recovers | Agent restart/ENOSPC tests drive the next attempt directly; desktop integration supervisor and native clipboard recovery still need native testing |
| Desktop UI and state | Not covered by containers | Chromium/WebKit mocked-Tauri tests, query/state unit tests |
| Native window/tray/startup, OS events and credential storage | Not applicable to Linux server fixture | Separate native lifecycle and Windows activation scripts; platform acceptance required |
| Application updates | Not covered by containers | Manifest/signature tooling and mocked UI; real installed-app upgrade required on each OS |

The remote suite is explicit and is not included in `npm run check` or hosted CI: the desktop backend requires macOS/Windows, while standard hosted macOS runners do not provide this OrbStack fixture. Agent CI provides all supported shells through Nix.

## Arboard compatibility

The fixture builds `tests/remote/arboard-probe` in a separate Rust Docker stage and copies only the executable into the Linux server. It pins **arboard 3.6.1**, with `image-data` and `wayland-data-control`, matching the existing agent compatibility suite. This is not a claim that every Codex release uses that exact dependency version.

Six remote tests run the same scenarios on X11 and Wayland. They isolate the backend by unsetting the other display variables, preventing fallback from hiding a failure. The probe uses an unmodified `arboard::Clipboard`, accepts synchronized `read-text`/`read-image` commands, emits JSON-line responses, and retains one clipboard instance throughout each sequence. Image assertions compare dimensions and SHA-256 hashes of decoded RGBA pixels, including alpha.

Coverage includes Unicode text, cached and changed reads, small and multi-chunk compressed PNGs, malformed images, empty clipboard, unsupported formats, a new copy during an in-flight image transfer, and restart without stale pixels. The restart scenario requires the old client to fail after the server stops, then starts a fresh probe with refreshed display variables. It does **not** promise an existing X11 connection can reconnect to a replacement server.

The host test supplies clipboard bytes over real SSH; it does not read or modify your actual clipboard. Run all scenarios with `npm run test:remote`.

## Fault scenarios and assertions

| Scenario | Test |
|---|---|
| Copy while an earlier paste is receiving chunks; then clear clipboard | `clipboard_copy_during_transfer_rejects_late_old_response` |
| Two simultaneous readers share the transfer | `clipboard_concurrent_readers_share_one_request` |
| SIGKILL the agent, restart, verify old data is unavailable | `crashed_agent_restarts_without_serving_old_clipboard` |
| Fill bounded tmpfs, assert retryable ENOSPC, free space and retry | `disk_full_reports_retryable_error_and_recovers_after_space_is_freed` |
| Kill SSH workers; automatically reconnect; cancel the next retry | `persistent_tunnel_reconnects_after_real_ssh_disconnect_and_stop_cancels_retry` |
| Cancel transfers after confirmed progress | `cancelled_upload_cleans_staging_and_can_be_retried`, `cancelled_download_preserves_destination_and_removes_partial_file` |
| Change the source after transfer progress | `changing_local_file_during_upload_does_not_publish_partial_content`, `changing_remote_file_during_download_preserves_local_destination` |
| Bad agent upload and managed version replacement | `agent_upgrade_and_bad_upload_preserve_working_installation` |
| Callback connection closes while destination remains healthy | `temporary_callback_does_not_reconnect_a_closed_ssh_session` |

The cancellation tests drop the production transfer future, as the operation registry does. They do not click a UI cancel button. Source-mutation tests synchronize on progress, avoiding guesses based on transfer speed. The old-version agent fixture is a marked stub, not a downloaded historical executable.

## Signed macOS upgrade

The opt-in `signed_macos_rebrand_installs_over_existing_bundle` test copies an installed Porthop bundle into a temporary directory, downloads and verifies a Newport update using the production updater, installs it in place, and checks its bundle identity, executable, signature, notarization ticket, and Gatekeeper acceptance. It does not modify the source app or launch the updated copy.

Set `NEWPORT_UPDATE_TEST_APP` to an installed Porthop `.app` and `NEWPORT_UPDATE_TEST_PACKAGE` to the signed, notarized Newport `.app.tar.gz` (with its adjacent `.sig`), then run:

```sh
cargo test --manifest-path src-tauri/Cargo.toml signed_macos_rebrand_installs_over_existing_bundle -- --ignored --nocapture
```

This covers package installation, not the Settings UI, restart, or migration of a user's live credentials. Those need a separate native upgrade check.

## Next scenarios

1. Native clipboard image → desktop supervisor → SSH → agent → arboard, including automatic crash/ENOSPC recovery.
2. Actual network interruption during a file transfer, and upgrade from a running historical agent binary.
3. Isolated systemd and Docker/Compose fixtures for successful discovery, logs and lifecycle actions.
4. Native macOS/Windows checks for credentials, sleep/network events and signed updates.
5. Callback expiry and hidden-callback detection against representative CLI flows.

Do not mark these scenarios covered merely because a corresponding mocked UI test passes.
