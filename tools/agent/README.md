# Newport agent (Needle)

One Linux binary provides remote metrics, Git, clipboard, and browser services.
Newport starts negotiated workers over SSH and owns their lifecycle. Initial
installation requires approval. For approved installations, Newport checks on
connection and hourly while connected, then automatically installs a newer signed
Needle bundled with the desktop. Ordinary feature access starts the installed build.

## Build and test

From the repository root, with Rust and Docker installed:

```sh
cargo xtask agents
cargo test --locked --manifest-path tools/agent/Cargo.toml
```

The build produces static x86_64 and ARM64 Linux binaries in `resources/agents`.
Desktop packaging compresses them into resources. Building agents replaces local
resource files; preserve signed artifacts when preparing an existing release.
Windows builds consume the Linux CI artifact instead of substituting stubs.

For a standalone Linux installation:

```sh
cargo install --locked --path tools/agent --root "$HOME/.local"
newport-agent install
```

## Terminal commands

| Command | Purpose |
| --- | --- |
| `sessions` | List live session identities and enabled integrations |
| `env [--shell SHELL]` | Print session exports for Bash, Zsh, sh, or Fish |
| `clipboard -o [-t FORMAT]` | Read a clipboard format |
| `open URL` | Open an HTTP(S) URL on the connected computer |
| `display --backend BACKEND -- COMMAND` | Run a command with an isolated X11 or Wayland clipboard display |
| `install` | Install owned aliases and shell configuration, preserving unrelated files |

See the [sharing guide](../../docs/clipboard.md) for shell setup and browser login
callbacks. Internal services include `runtime --stdio`, `git-rpc --stdio`,
`monitor --stdio`, and the managed integration worker. The desktop negotiates
capabilities and build identity before starting workers.

## Sessions and permissions

`NEWPORT_SESSION` selects the desktop session. Routing history reconciles old
identities with a newer connection from the same client. Recovery of an unknown
old selector requires an unambiguous eligible destination; ambiguity or disabled
permission fails rather than silently choosing another computer. Without a
selector, exactly one eligible session is required.

Browser and Clipboard switches change acknowledged runtime permissions without
reinstalling helpers or reconnecting SSH. Disabling cancels local access and
pending requests immediately. The runtime also serves other features, so turning
off sharing does not mean that all agent services stop.

Runtime directories are private and use OS lifetime locks. Startup sweeps only
recognized abandoned directories; unknown or ambiguous paths are retained.
Installation admission uses `~/.local/state/newport/usage` and does not require
writable `/tmp`.

## Deployment and recovery

Managed builds live under
`~/.local/share/newport/needle/builds/SHA256/newport-agent`. Newport verifies the
candidate handshake, serializes activation, and atomically switches the command
symlink. Existing workers retain their negotiated build.

Automatic upgrades require verified release ordering and never downgrade or
replace an unknown build. They defer while another lifecycle operation is active
or recovery is unfinished. Failed attempts are throttled; use the Integrations
page for manual updates and recovery. Checks do not reconnect disconnected servers
or reinstall a removed agent.

Helper writes are preflighted and journaled with ownership evidence and backups.
Recovery preserves unrelated edits; conflicts require review. Durable operation
IDs let retries observe previous outcomes without repeating uncertain writes.
Removing an installation requires settled operations and no active services;
retained builds and backups support restoration.

The `deployment` commands implement preparation, activation, operation lookup,
reconciliation, helper review, recovery, removal, and restoration. Their protocol
and compatibility checks are defined in [deployment.rs](src/deployment.rs).
Use the Integrations page for ordinary lifecycle changes. Do not delete ownership
journals, backups, or usage locks as a repair shortcut.
