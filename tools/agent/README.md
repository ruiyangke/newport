# Newport agent (Needle)

One Linux binary provides remote metrics, Git, clipboard, and browser services.
Newport starts negotiated workers over SSH and owns their lifecycle. Initial
installation requires approval. After that, Newport checks on connection and
hourly while connected. Automatic updates stay within the installed product
variant and preserve newer signed releases. A core desktop can use core services
of a full agent without replacing it. A desktop's runtime starts that desktop's
own build when it is staged on the server, and the installed build otherwise.

## Composing another executable

GUI handoff service versions 1 and 2 retain their existing strict wire records.
Version 3 permits a helper to request `sessionBus` and receive an optional
`fallbackBus` in the shared environment. The desktop omits that field for older
helpers, including older helpers connected to a version-3 broker. Use the
fallback only when both inherited session/starter bus addresses are empty;
validate its local socket before executing the waiting command. The display
owner supervises this bus. It is not a separate per-command daemon.
Version 4 adds native Wayland displays (explicit null X11 endpoints). Version 5
permits a helper to request `applicationEnvironment` and receive `application`:
the display's application policy (`contract::application_v1`:
`XDG_CURRENT_DESKTOP`, `XDG_DATA_DIRS`, `MOZ_ENABLE_WAYLAND`,
`ELECTRON_OZONE_PLATFORM_HINT`), applied like the toolkit settings. Version 6
lets a helper also request `applicationEnvironmentV2`, receiving
`contract::application_v2`, which adds `GSETTINGS_SCHEMA_DIR`, `GTK_A11Y` and
`NO_AT_BRIDGE`; a version-5 helper gets only the `application_v1` names. The
desktop omits the policy for older helpers, as it does `fallbackBus`.

An embedding product can depend on this public library and call `cli::run` with
its compiled `composition::Service` implementations. Core services cannot be
replaced. Extension workers use the same bounded registration, exact executable
hash, session identity and lifetime lease as core workers. A `WorkerContext`
retains that lease and exposes read-only cancellation; the service owns bounded
I/O, child supervision and cleanup.

`WorkerContext::clipboard()` reads authorized host imports or a local native selection through
the runtime's local service. Metadata contains a runtime identity and revision;
`Reader::read` / `Read::next` fetch a selected format only on demand, in chunks
of at most 64 KiB with a 32 MiB payload limit. Run these blocking calls outside
display and transport event loops. Command helpers and admitted workers share
the same host fetch and cache. Revoking sharing or retiring the integration
worker withdraws its host offer and cancels those reads; retiring another worker
revokes only that worker's access. This API grants no clipboard export and
starts no display or SSH channel.
For a paste handled by another task, create `ReadCancellation` first and pass
it to `read_with_cancellation`. Cancelling interrupts authentication or a
blocked payload read without affecting other consumers; a pending connect
remains bounded to two seconds. Each handle belongs to one read.

Native selection providers use `WorkerContext::native_clipboard()`. Runtime
control must explicitly assign an admitted worker using `ClipboardRoute`,
negotiated through `clipboard-service` v1. Publication compares the last seen
runtime revision, uses a provider lease and consumes stale native revisions
so they cannot be retried against a newer host copy. Helpers and native clients
share one demand cache. Repeated host metadata does not overwrite a newer
native selection; disabling host imports preserves local native data.

The route's export permission and focus are separate from import sharing.
Only the integration producer can obtain an export offer and read it under
that exact lease. Losing focus, permission, provider ownership or producer
lifetime rejects stale exports and interrupts pending export reads without
cancelling local readers. The desktop must also check its own focus and native
clipboard change count immediately before writing. No host export packets are
emitted by this API alone. Pure contract types live in `clipboard_contract.rs`,
which the desktop can include without importing the agent's IPC implementation.

The executable dispatches its own guardian and application helper modes before
entering the shared CLI or starting threads. Core has no dependency on a private
product. `tests/composition.rs` builds an isolated consumer offline and exercises
admission, helper re-execution, worker failure and runtime-scoped cancellation.

`product --json` reports the executable hash, product variant and compiled
control/service versions independently of host dependencies or account access.
Composition hosts select their variant through `cli::run_product`. Runtime v1
advertises this identity through a capability name, retaining its existing
strict handshake fields. Signed schema-2 manifests bind the same protocols to
the release hashes; existing schema-1 releases identify core.

Deployment checks the expected active hash under the installation lock. A full
agent cannot be replaced with core unless `deployment install` receives the
explicit `--allow-downgrade` option. Newport supplies `--expect-current` and
`--expect-variant` after its product probe; unknown/malformed identities stop
replacement. These checks cannot constrain old desktop binaries that overwrite
user-owned files without using this installer.

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
workers keep their negotiated build. With `--unless-running`, the entry point
stays on its build while a process runs that build, checked under the
installation lock; the result's `active` field says whether the entry point is
the new build. A runtime start marks its build folder as used. Afterwards, and
when a runtime starts, builds are removed unless they are the active build, a
process still runs them, they were staged or started within the last hour, or
they are one of the two most recently used of the rest and were used within
30 days. On systems without `/proc`, no build is removed. Journals that older Needle versions kept are deleted. A
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

Needle follows Newport: once installing is approved, Newport checks the
installed build on connection and hourly. Several desktops can use one server,
each with its own build, so the automatic update decides two things apart:

- The desktop's own runtime. Its build is staged and verified beside the others
  in `builds/`, and the runtime moves to it without stopping admitted work.
  New runtimes start it directly. Over a newer signed release at the entry
  point, nothing is staged: that release serves the older desktop.
- The entry point, which terminals, command links and desktops without a staged
  build use. It follows signed release order: an older release is replaced, a
  newer one never (Newport says to update Newport). Builds without an order,
  such as development builds or two builds of one release, replace each other
  only when no process runs the installed build: the check lists the builds
  that this account's processes run from `/proc`, and `deployment install
  --unless-running` checks again under the lock. Where processes can't be
  listed, the entry point stays. So two connected desktops never take turns:
  the entry point moves at most once, after the other desktop has left.

A Remote Apps launch on a runtime of another build first moves it to this
desktop's build under the same rules. Failed attempts are throttled; use the
Integrations page to install, update or uninstall by hand. Installing by hand
always points the entry point at this desktop's build. Checks do not reconnect
disconnected servers or reinstall an uninstalled agent.
