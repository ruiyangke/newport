# Newport agent (Needle)

One Linux binary provides remote metrics, Git, clipboard, and browser services.
Newport starts negotiated workers over SSH and owns their lifecycle. Initial
installation requires approval. After that, Newport checks on connection and
hourly while connected, and replaces any Needle that isn't its own build with the
one it bundles. Ordinary feature access starts the installed build.

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
| `install` | Write the command links and shell setup, leaving unrelated files alone |

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

## Install and uninstall

A server has Needle installed or not; there is nothing in between.

Installing (also updating) stages the build under
`~/.local/share/newport/needle/builds/SHA256/newport-agent`, verifies its
handshake, and runs `deployment install SHA256 [--integrations]`, which points
`~/.local/bin/newport-agent` at it and, with integrations, writes the command
links and the shell setup block. It is idempotent: running it again changes
nothing, and an interrupted install is finished by installing again. Existing
workers keep their negotiated build. Afterwards, and when a runtime starts,
older builds are removed unless they are the active build, a process still runs
them, or they were staged within the last hour. On systems without `/proc`, no
build is removed. Journals that older Needle versions kept are deleted. A
Needle installed as a plain file at `~/.local/bin/newport-agent` (by hand, or
before builds were managed) is recognized by the protocol name it contains and
replaced; any other file there is left alone and the install stops.

Installing also removes the old Porthop agent and its marked helper scripts.
It scans every symlink directly in `~/.local/bin` and removes those pointing to
Porthop's agent or helpers, including custom names, dangling links, and chains
of links within that directory. It stops the old agent before deleting its
clipboard cache and removes Porthop's shell setup blocks and shell setup lock.
Unrelated commands and files
are preserved. This cleanup runs even without `--integrations`; with integrations,
the standard helper names receive Newport's links. Staging alone leaves Porthop
in place until the new build has been verified and activated.

Shell setup is a block between `# >>> Newport >>>` and `# <<< Newport <<<`,
written atomically with the file's permissions. A startup file that is a
symlink or read-only is left alone, and the person is told what to add by hand.
Another `xclip`, `xsel`, `wl-paste` or `xdg-open` stays; `newport-browser` must be free.

Uninstalling is Newport's job and needs no Needle executable: it runs
[agent-uninstall.sh](../../crates/newport-backend/src/agent-uninstall.sh) over
SSH, which stops Needle's processes, removes the shell setup blocks, the
links pointing at Needle and a plain-file Needle at its entry point, and deletes `~/.local/share/newport`,
`~/.cache/newport`, `~/.local/state/newport` and the runtime directory. It can't
be undone; installing again starts from nothing.

Needle follows Newport: once installing is approved, any Needle that isn't
Newport's own build is replaced automatically, on connection and hourly. The one
exception is a signed release known to be newer than Newport's, which would be a
downgrade; Newport says to update Newport then. Failed attempts are throttled; use the
Integrations page to install, update or uninstall by hand. Checks do not
reconnect disconnected servers or reinstall an uninstalled agent.
