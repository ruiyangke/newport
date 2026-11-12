# Newport agent (Needle)

One Linux binary provides remote metrics, Git, clipboard, and browser services.
Newport starts negotiated workers over SSH and owns their lifecycle. Initial
installation requires approval. After that, Newport checks on connection and
hourly while connected. Automatic updates stay within the installed product
variant and preserve newer signed releases. A core desktop can use core services
of a full agent without replacing it. A desktop's runtime starts that desktop's
own build when it is staged on the server, and the installed build otherwise.

## Composing another executable

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
emitted by this API alone. Shared contract types live in `newport_protocol::clipboard`,
which the desktop can include without importing the agent's IPC implementation.

The executable dispatches its own guardian and application helper modes before
entering the shared CLI or starting threads. Core has no dependency on a private
product. `tests/composition.rs` builds an isolated consumer offline and exercises
admission, helper re-execution, worker failure and runtime-scoped cancellation.

`--version` reports the semantic product release from `resources/app.json`,
shared by Newport and both Needle variants. `product --json` reports that
version, the executable hash, product variant and compiled control/service
versions independently of host dependencies or account access. The runtime
announcement carries the product release as `agentVersion`; protocol versions
remain separate. Hashes identify exact executables for verification and retained
sessions. Signed release metadata determines release order.
Composition hosts select their variant through `cli::run_product`. Runtime v1
announces an explicit product and a map of service versions and availability
through the shared `newport_protocol::runtime` protobuf codec. Worker admission
and leases use the same crate; each control reply carries one result or error. Signed schema-2 manifests bind the same protocols to
the release hashes; existing schema-1 releases identify core.

Deployment checks the expected active hash under the installation lock. A full
agent cannot be replaced with core unless `deployment install` receives the
explicit `--allow-downgrade` option. Newport supplies `--expect-current` and
`--expect-variant` after its product probe, which identifies the installed
build by its explicit product variant, whatever its services speak. The install
takes the installed build's variant from its `product --json` and checks only
that identity's schema version and hash, and that the hash is the
executable's checksum. Output that doesn't parse as an identity, such as an
unknown variant or field, or one failing these checks stops the install; the
rest of the product, such as its services, isn't checked there.

Newport identifies this way only an installed build without release metadata.
When it falls short of what this Newport needs, and every version it offers of
each such service is below the one needed, such as a full agent with only an
earlier Remote Apps version, it is an older build of its variant: Newport
reports it as needing an update and replaces it as it replaces an older
release. Any other build it can't use may be newer, and is replaced only while
no process runs it, as other unordered builds are. Remote apps reports a newer
Newport Remote when each such service offers a later version and one of them is
what Remote apps needs, a newer agent when none of them is, and otherwise that
the build doesn't work with this Newport. Runtime announcements must name a
recognized product and may not name a service twice. An undecodable announcement
cannot override verified release identity or authorize a product downgrade.
These checks cannot constrain
old desktop binaries that overwrite user-owned files without using this
installer.

## Build and test

From the repository root, with Rust and Docker installed:

```sh
cargo xtask agents
cargo test --locked --manifest-path crates/newport-agent/Cargo.toml
```

The build produces static x86_64 and ARM64 Linux binaries in `resources/agents`.
Desktop packaging compresses them into resources. Building agents replaces local
resource files; preserve signed artifacts when preparing an existing release.
Windows builds consume the Linux CI artifact instead of substituting stubs.

For a standalone Linux installation:

```sh
cargo install --locked --path crates/newport-agent --root "$HOME/.local"
newport-agent install
```

## Terminal commands

| Command | Purpose |
| --- | --- |
| `env [--shell SHELL]` | Print Needle's exports for Bash, Zsh, sh, or Fish |
| `clipboard -o [-t FORMAT]` | Read a format from the clipboard of the computer the request goes to; `-t TARGETS` lists them |
| `open URL` | Open an HTTP(S) URL on the connected computer |
| `display --backend BACKEND -- COMMAND` | Run a command with an isolated X11 or Wayland clipboard display |
| `display --terminal ID -- COMMAND` | Become COMMAND, a plain Newport terminal's shell, with the terminal's clipboard display |
| `terminal display` | Print the account's display names, as a held shell gets them, for a terminal's shell to evaluate |
| `install` | Write the command links and shell setup, leaving unrelated files alone |

See the [sharing guide](../../docs/clipboard.md) for shell setup and browser login
callbacks. Internal services include `runtime --stdio`, `git-rpc --stdio`,
`monitor --stdio`, and the managed integration worker. The desktop negotiates
capabilities and build identity before starting workers. The integration worker
uses `newport-protocol::integration` for numbered requests, responses and typed
clipboard/browser events. Runtime-bound permissions travel on the independent
worker lease connection, so a stalled feature transfer cannot delay revocation.
Standalone managed sessions acknowledge permissions on their integration stream.

The local browser helper uses `newport-protocol::browser` and its generated
protobuf encoding. It verifies the owner's announcement before submitting a
URL, sends one request, and closes its write side. The owner checks permissions
and accepts at most one pending open. Deadlines bound the entire exchange.
An uncertain opening result is not retried automatically.
The runtime advertises the version-1 `browser-helper` capability. Newport checks
that capability on the permission-owning runtime before enabling browser access.

The Git worker uses `newport-protocol::git` for configuration, typed requests,
operation receipts and results. Its protobuf transport sends diff bodies in
acknowledged chunks. The executor and durable journal belong to the agent;
shared validation and UI projections belong to the public protocol crate.

## Sessions and permissions

A request from a Newport terminal goes to the computer the terminal is used on:
a held shell's active computer, or the one a plain terminal recorded with
`terminal route` before its shell started. A program on the account's stable
display goes to the computer viewing it. Routing history follows a computer to
its newer connection from the same client. Anything else needs exactly one
eligible connection; ambiguity or disabled permission fails rather than
silently choosing another computer.

Browser and Clipboard switches change acknowledged runtime permissions without
reinstalling helpers or reconnecting SSH. Disabling cancels local access and
pending requests immediately. The runtime also serves other features, so turning
off sharing does not mean that all agent services stop.

Runtime directories are private and use OS lifetime locks. Startup sweeps only
recognized abandoned directories; unknown or ambiguous paths are retained.

## Terminal clipboard displays

Every Newport terminal names a display, so the server's own clipboard commands
(`xclip`, `xsel`, `wl-paste`, `wl-copy`) and programs that use X11 or Wayland
directly work in it: the account's display when Remote apps run one, or else a
clipboard display of the terminal's own, with X11 and Wayland names where each
can start. A held shell's holder serves it; a plain terminal's shell runs under
`display --terminal`, which serves it from a child in its own session and
becomes the shell, so job control, signals and the exit status stay the
shell's. It ends when the shell does, and the shell starts without it if it
can't. A shell Newport didn't start has no implicit clipboard display; the
explicit command there is `newport-agent clipboard`.

On such a display, CLIPBOARD and PRIMARY (X11's PRIMARY, Wayland's primary
selection through wlr-data-control) both offer the clipboard of the computer
the terminal follows, chosen at each read. Text a program puts on CLIPBOARD is
taken from it and written to the terminal as an OSC 52 copy, which the terminal
applies on the computer it is shown on; the program then loses the selection,
which is the computer's clipboard again. A program's PRIMARY is taken back the
same way and never sent. Other displays, such as `display --backend`, refuse
copies.

Wayland clipboard clients can probe compositor globals without requesting GUI
setup. Surface and region requests still signal GUI intent; the clipboard
display refuses them and keeps serving clipboard clients.

## Install and uninstall

A server has Needle installed or not; there is nothing in between.

Installing (also updating) stages the build under
`~/.local/share/newport/needle/builds/SHA256/newport-agent`, verifies its
handshake, and runs `deployment install SHA256 [--integrations]`, which points
`~/.local/bin/newport-agent` at it and, with integrations, writes the command
links (`xdg-open` and `newport-browser`) and the shell setup block. Owned browser
links point directly to the verified immutable build. Their stable paths remain
usable from running shells when the default agent stays installed.
It is idempotent: running it again changes
nothing, and an interrupted install is finished by installing again. Existing
workers keep their negotiated build. With `--unless-running`, the entry point
stays on its build while a process runs that build, checked under the
installation lock. `--keep-default` preserves the default regardless of running
processes; both modes refresh owned helper links. The result's `active` field
says whether the entry point is the new build. A runtime start marks its build
folder as used. Afterwards, and
when a runtime starts, builds are removed unless they are the active build, a
process still runs them, an owned browser link references them, they were staged
or started within the last hour, or
they are one of the two most recently used of the rest and were used within
30 days. On systems without `/proc`, no build is removed. Journals that older Needle versions kept are deleted.
`~/.local/bin/newport-agent` is Newport's entry point: a plain file or a link
anywhere else there, such as a Needle copied by hand, is replaced. Only a
directory there stops the install.

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
Another `xdg-open` stays; `newport-browser` must be free. Every install removes
links named `xclip`, `xsel` or `wl-paste` that point at `newport-agent`, which
would shadow the server's own clipboard commands; a file of that name that
isn't such a link stays.

Uninstalling is Newport's job and needs no Needle executable: it runs
[agent-uninstall.sh](../../crates/newport-backend/src/agent-uninstall.sh) over
SSH, which stops Needle's processes, removes the shell setup blocks, the
links pointing at Needle and whatever is at its entry point (a link, never its
target), and deletes `~/.local/share/newport`,
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
