# Newport agent

One Linux binary for Newport's remote metrics, Git, clipboard, and browser services.

Newport installs or upgrades Needle only through an explicit setup or lifecycle action. Feature access starts an already-installed service over the shared server session. It provides read-only X11 and Wayland clipboard services, `xclip` / `wl-paste` aliases, and an `xdg-open` alias that opens headless web requests on your computer. See the [runtime architecture](../../docs/needle-architecture.md) and [clipboard guide](../../docs/clipboard.md).

Browser and Clipboard switches change acknowledged runtime permissions. They do not reinstall helpers or reconnect SSH. Disabling revokes local access immediately, cancels pending reads or opens, and removes the affected temporary resources. Enabling becomes active after Needle acknowledges the permission revision. Older agents without the managed protocol require an explicit update.

## Build

With Rust and Docker installed, run from the repository root:

```sh
npm run build:agent
```

This builds static x86_64 and ARM64 Linux binaries in Docker, then places them in `resources/agents` for embedding in the shared backend. Desktop development and release builds run this step automatically. Run it before invoking Cargo directly for the backend or shipping host.

For a local build and tests:

```sh
cargo test --locked --manifest-path tools/agent/Cargo.toml
```

To install from source on Linux:

```sh
cargo install --locked --path tools/agent --root "$HOME/.local"
newport-agent install
```

Registered integrations use private directories per runtime. Browser and clipboard helpers honor `NEWPORT_SESSION`; they never fall back from a stale explicit selection to another desktop. Without a selection, exactly one eligible session must exist. Newport supplies this identity when opening a terminal, before the login shell reads its startup files.

New runtime, integration, and temporary display directories carry private lifetime markers. The control process and its workers hold shared OS file locks; deletion requires exclusive ownership. The last user normally removes the directory, and subsequent startup performs a bounded sweep of abandoned marked directories after a crash. Registry publication has its own lock, so a live bridge remains registered even while both integrations are disabled. Unknown, legacy unmarked, or non-private directories are retained.

X11 binds its socket in a private directory and records ownership before publishing the display address without overwriting an existing path. Its authority file stays inside that directory. Cleanup requires the original socket inode, no real X server lock, and a refused connection; active or ambiguous endpoints are retained for a later attempt. Runtime startup performs this recovery even with Clipboard disabled. Temporary standalone Wayland displays use the same directory lifetime protection.

## Managed deployment

Service CLI entry points hold shared usage leases scoped to the UID and canonical HOME, independently for controllers and workers. Destructive maintenance can acquire an exclusive lease only when those services have exited; a held maintenance lease rejects new services before they emit protocol output. Coordination lives in a private `/tmp/newport-use-*` directory and does not install executables or modify HOME. Its lock file is deliberately retained to avoid splitting ownership across replaced inodes. The lifecycle probe advertises `usageLease: 2`, including persistent admission blocking during and after removal. Removal refuses retained builds without verified admission support.

Newport uploads into a temporary file and prepares a content-addressed build under `~/.local/share/newport/needle/builds/SHA256/newport-agent`. It verifies that candidate's actual runtime handshake before activating it. A private, OS-owned file lock serializes activation across clients; activation compares the installed checksum with the inspected checksum and refuses a concurrent change. The active command is switched by atomic symlink rename. Recognized legacy binaries and previous builds are retained; running workers continue using their negotiated build.

Helper configuration failure attempts to restore the previous entry point while holding that lock. Journaled activation retains a needs-resolution receipt because executable rollback does not prove that shell changes were restored. Durable operation IDs preserve activation outcomes across interruption; retry observes the same receipt without repeating helper writes. Approved desktop handoff drains old channels between requests. Alias, legacy-command, and shell changes now use a private ownership journal and checksum-verified backups. Installation preflights all targets, propagates shell errors, and attempts guarded rollback on write failure. Later edits survive rollback; interrupted or conflicting helper transactions block further helper installation. A matching applied helper receipt lets activation reconciliation confirm completion without repeating those writes. The native Integration page now exposes explicit removal confirmation, durable recovery actions, and removal-restoration file review. Initial removal-planning conflicts, older-build migration, release ordering, receipt retention, build garbage collection, and general repair remain open. No ordinary feature launch deploys a build.

A first journaled binary-only deployment records an empty helper ownership journal before activation without editing aliases or shell files. Existing helper history is preserved. Removal and restoration can therefore handle a new installation with no integrations through the same durable transaction. Missing or corrupt history in an existing helper directory is never treated as empty, and later reinstalls do not recreate it; older installations without ownership evidence still require migration or repair.

## Commands

- `runtime --stdio`: bounded control handshake, exact running build identity, capability/dependency discovery, initialization, worker registration, acknowledged permission revisions, health, and shutdown.
- `worker BUILD_SHA256 RUNTIME_ID SOCKET_PATH WORKER_ID git|monitor|integration [CLIENT_ID]`: internal launcher that verifies the negotiated executable and consumes a one-use registration from the intended runtime before entering the feature protocol. Its private socket binds worker lifetime and permission control to that runtime; a matching binary alone does not authorize attachment. Registered integration workers accept permission updates only through this control path.
- `serve CLIENT_ID [--clipboard] [--browser]`: framed clipboard and browser transport on stdin/stdout, managed by Newport over SSH.
- `serve CLIENT_ID --managed`: versioned integration handshake and live permission updates; starts with both permissions disabled. This remains the standalone integration entry point. Newport starts negotiated workers through the runtime launcher; the flag-based mode remains available for legacy protocol clients.
- `git-rpc --stdio`: independent Git request service.
- `monitor --stdio`: metrics and server inspection service.
- `install`: install aliases, preserving unrelated commands.
- `deployment helper-history`: read-only JSON discovery of standalone helper operations, including origin, phase, file count, ownership fingerprint, and any restoration blocker. Requires no managed deployment metadata and creates none. Records without provenance remain unattributed; deployment-owned operations are excluded. The protocol probe advertises `helperHistory: 1` and `standaloneHelperRecovery: 1`.
- `deployment restore-helper UUID OWNERSHIP_FINGERPRINT`: explicitly restore one verified standalone helper operation, preserving unrelated edits and refusing later overlapping ownership. Does not replace the executable. Retries of completed restoration preserve subsequent changes.
- `deployment review-helper UUID OWNERSHIP_FINGERPRINT` and `deployment keep-helper UUID OWNERSHIP_FINGERPRINT INDEX FILE_FINGERPRINT`: inspect restoration conflicts and explicitly keep a reviewed current file. Ownership and file fingerprints protect different decisions; a changed file requires a fresh review. Follow with `restore-helper` to finish restoration. Desktop adoption and recovery actions for this history remain pending.
- `deployment prepare HASH`, `deployment activate HASH EXPECTED [--integrations]`, `deployment rollback HASH PREVIOUS`: internal explicit lifecycle operations. `prepare` never activates; the desktop must validate the candidate before activation. `EXPECTED` and `PREVIOUS` can be `missing` for first installation.
- `deployment activate HASH EXPECTED --operation UUID [--integrations]`: journaled activation with a stable operation ID and JSON outcome. Duplicate IDs must have identical parameters and never repeat the mutation. Unsettled receipts block another activation.
- `deployment operation UUID`: read-only JSON observation of a saved receipt and the active build. `deployment reconcile UUID` updates receipt metadata when binary activation is proven; it never replays activation or uncertain helper changes. The private versioned journal retains up to 1,000 receipts within 1 MiB and preserves corrupt or unknown data instead of replacing it.
- `deployment recover UUID`: explicitly restore the selected deployment’s recorded helper changes and previous entry point. Recovery persists its own phase, preserves conflicting edits, and refuses newer deployments or helper transactions. Repeating a completed recovery observes its receipt without undoing later work. Newport exposes this action for unfinished deployments on Integration; general uninstall is separate.
- `deployment review UUID`: read-only file review for a pending restore. Reports recorded paths and their status, without returning file contents or writing backup objects.
- `deployment keep UUID INDEX FINGERPRINT`: explicitly preserve the reviewed conflicting path and exclude it from this restore. Rejects stale fingerprints and records accepted decisions durably. A later `recover` restores remaining paths; kept files may retain Newport commands.
- `deployment helper-removal-plan`: read-only helper removal planning for a managed installation. Composes applied helper history, respects keep decisions, and reports remove/restore/unchanged/kept/conflict per path. Missing ownership evidence or unsettled operations block planning. It does not remove files, check active clients, or authorize uninstall. The read-only protocol probe advertises `helperRemovalPlan` when supported.
- `deployment remove BUILD --operation UUID`: explicitly remove recorded helpers and the active entry point from a compatible managed installation. Requires no active services, settled deployment receipts, helper ownership evidence, and supported lifecycle metadata for every retained build. Persists intent before writes and blocks future service admission. Keeps immutable binaries and backups for restoration. Conflicts and older builds are preserved rather than forced through removal.
- `deployment removal UUID`: read-only observation of a durable removal record. Duplicate removal IDs never repeat completed writes or undo a later restoration.
- `deployment restore-removal UUID`: explicitly restore the recorded helpers and executable entry point, then reopen service admission. Later edits or missing evidence leave recovery pending. A completed restore is not replayed over a subsequent removal. The desktop lifecycle owner exposes confirmation and recovery on Integration; older-build migration remains incomplete.
- `deployment restore-removal UUID --build HASH`: restore the matching removal or durably cancel an undispatched intent. Cancellation records a restored receipt under the activation lock, preventing a delayed command with the same ID from removing files. It does not replay file restoration for a completed ID. The lifecycle probe advertises `removalCancellation` for this behavior.
- `deployment review-removal UUID --build HASH`: read-only JSON file review for the current pending removal restoration, using its recorded build and operation identity. Reports paths, status, and eligible conflict fingerprints without returning file contents or restoring files. The lifecycle probe advertises `removalReview`; unsupported recorded builds preserve recovery evidence and refuse the request.
- `deployment keep-removal UUID --build HASH INDEX FINGERPRINT`: durably preserve a reviewed conflicting path and exclude it from this removal restoration. Rejects stale fingerprints and mismatched or obsolete operations; an accepted decision is idempotent. Follow with `restore-removal` to restore remaining paths and the entry point. Kept paths release earlier installation ownership, so later removal planning does not reclaim them unless a later installation establishes new ownership. On Integration, “Review files” and “Keep current file” expose these decisions during an unfinished restore; keeping a file leaves its Newport shell configuration untouched.
- `env`: print the selected integration environment and pin `NEWPORT_SESSION` for that shell.

- `sessions`: list live session identities and enabled features. Set `NEWPORT_SESSION` explicitly when multiple desktops are connected.
- `clipboard -o [-t FORMAT]`: read a snapshot format.
- `open URL`: open an HTTP(S) URL on your Mac, optionally forwarding a login callback.
- `display --backend x11|wayland -- COMMAND`: run a command with an isolated clipboard display.

New shell ownership records distinguish the managed block from the surrounding file. Recovery can remove or restore an unchanged recorded block while preserving later surrounding edits and writable permissions. Changes inside the block, malformed markers, read-only files, and replaced symlinks remain conflicts. Older whole-file receipts retain their original exact-match recovery policy. Restoring shell helpers does not replace the Needle executable or stop its runtime.

The persistent transport is versioned and size-bounded. It uses atomic snapshots, private local sockets, exclusive ownership, and a heartbeat timeout. Browser logins with an explicit HTTP loopback `redirect_uri` get a temporary listener on the same Mac address and port, forwarded over the existing SSH connection. `localhost` binds both IPv4 and IPv6. Listeners expire after 5 minutes or when Integration disconnects. Forwarding failures produce a warning while allowing the browser to open.

Device-code URLs need no forwarding. Other callback schemes and container-private listeners are not supported automatically. Newport preserves the login URL and leaves token exchange to the remote CLI.
