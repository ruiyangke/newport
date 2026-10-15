<p align="center">
  <img src="resources/icons/icon.png" width="88" alt="Newport app icon" />
</p>

<h1 align="center">Newport</h1>

<p align="center"><strong>Make your remote server feel closer.</strong></p>

<p align="center">
  SSH tunnels, terminals, files, and server health in one desktop app.<br />
  Copy locally. Paste remotely. Open server links in your own browser.
</p>

<p align="center">
  <a href="https://github.com/ruiyangke/newport/releases"><strong>Download Newport</strong></a> ·
  <a href="docs/README.md">User guides</a> ·
  <a href="https://github.com/ruiyangke/newport/issues">Feedback</a>
</p>

## Your server, within reach

Newport is a desktop workspace for your Linux servers, available on macOS and Windows. Save a connection once, then move between your server’s apps, terminal, files, and activity without juggling separate tools.

### Open remote apps locally

Reach a development server, dashboard, or database through an SSH tunnel. Listening services are discovered automatically and grouped by project (Git — with each worktree under its branch — Compose, or the project root around each process), tagged with what they run. Forward one port in a click, or a whole project at once. Reverse forwards share a port on your Mac with the server. Automatic reconnect helps recover from temporary interruptions.

### Copy here. Paste there.

Copy text or a screenshot on your computer and paste it into a supported remote application, including Codex. It works even when the server has no graphical desktop. Your clipboard is shared when a remote app requests it.

### Sign in with your own browser

When a remote command opens a link, Newport can open it on your computer. Complete browser-based sign-ins without copying URLs between machines. Supported login callbacks are forwarded back to the server; some services still need a manual step.

### See how your server is doing

Check CPU, memory, storage, and network usage at a glance. Explore up to seven days of history to spot busy periods, spikes, and changes over time.

### Work with terminals and files

Open an interactive SSH terminal that reconnects automatically if the connection drops and starts a fresh shell. Browse remote folders, preview files, and upload or download over SFTP—all from the same server workspace. Use tmux when you want remote work to continue after leaving the terminal.

### Manage services and containers

See what needs attention first: failed systemd services and unhealthy or crashed containers, with the reason in plain words. Follow logs live, start, stop, and restart services and containers, read unit files, and inspect a container’s environment, mounts, and networks. Open a shell inside a container, forward its published port and open it in your browser in one click, and see what Docker’s images, volumes, and build cache take on disk.

## Get started

1. **[Download the app](https://github.com/ruiyangke/newport/releases)** for your computer.
2. **Add a server.** Enter its SSH host and username, then choose a key file, SSH agent—including 1Password—or password.
3. **Set up your server.** Overview inspects the server, asks permission to install Needle, and lets you choose clipboard and browser sharing. You can skip installation and use features that do not require the agent.
4. **Start working.** Open a terminal, browse files, or add your first tunnel from the sidebar.

| Your computer                              | Download                 |
| ------------------------------------------ | ------------------------ |
| Mac with Apple silicon · macOS 14 or later | macOS ARM64 ZIP          |
| Windows PC with an Intel or AMD processor  | Windows x86_64 installer |
| Windows PC with an ARM processor           | Windows ARM64 installer  |

Windows support is experimental, and its installers are not yet Authenticode-signed. Linux is supported as a remote server, not as a desktop app.

**Coming from Porthop?** Newport is its new name. Keep your existing app data and OS credentials when upgrading. Newport adopts the existing profile folder in place; do not rename it manually. See [storage and recovery](docs/storage.md) for migration, recovery snapshots, and downgrade limitations.

## Set up clipboard and browser sharing

Approve Needle installation in server setup, then enable **Clipboard**, **Browser**, or both. You can change these choices later in **Integration**. Open a new remote shell afterward, or follow the instructions on that page to use your current session.

Keep Newport running while sharing. You can switch either feature off independently, at any time. Clipboard sharing goes from your computer to the server.

Only enable sharing for servers you trust: remote apps can read sensitive content you copy or request browser tabs while these features are on.

[Read the integration guide →](docs/clipboard.md)

## Made for everyday use

- **Stay connected in the background.** Close the window and keep tunnels running, with access from the menu bar or system tray. Quit the app to disconnect.
- **Choose your appearance.** Use a light or dark theme and optionally launch Newport at login.
- **Update when you’re ready.** The native release candidate keeps automatic updates disabled until signed installation and upgrade acceptance is complete. The update flow verifies downloaded packages and requires a restart to apply them; restarting disconnects sessions and cancels transfers.
- **Keep credentials on your computer.** Saved profiles and passwords are encrypted, with the vault key stored in macOS Keychain or Windows Credential Manager.

Metric history is stored separately and is not encrypted. Clear it anytime in **Settings → Cache**. See [storage and recovery](docs/storage.md) and [security notes](docs/security.md) for backup requirements and known limitations.

## Before you connect

Newport uses your SSH account’s permissions. Service management needs systemd (changing system services needs root or sudo), and container features need access to Docker or Podman. Monitoring, Git operations, services, containers, clipboard, and browser sharing use Needle, the server agent. Newport asks permission before installing it; opening a page does not install the agent. Installation and upgrade status live in the server's agent controls, while clipboard and browser permissions remain separate choices.

SSH configuration aliases, ProxyJump, host certificates, and interactive MFA are not currently supported. Container actions manage existing containers; they do not deploy or recreate Compose projects.

## Help shape Newport

Found a bug or have an idea? [Open an issue](https://github.com/ruiyangke/newport/issues). For bugs, include your app version, operating system, and steps to reproduce, with private information removed from any logs.

Contributions are welcome. The [testing guide](docs/testing.md) covers local checks and tests against disposable Linux servers.

## Native development

The root Cargo workspace owns the standalone `newport-app`, GPUI interface,
shared backend/client/platform services, grammars, and release tooling. The Tauri
host has been removed from the source tree and CI; native development no longer
requires a running Tauri application. Build output lives in `target/`.

Shared application models and pure rules live in
[`newport-domain`](crates/newport-domain/README.md). Desktop IPC contracts and OS
adapters belong to `newport-platform`; feature runtime state stays with its owner.

```sh
node scripts/build-grammars.mjs
NEWPORT_DATA_DIR=/absolute/path/to/test-profile npm run dev:native
npm run test:native
```

See the [standalone development guide](crates/newport-app/README.md) for Windows
dependencies and packaging. Debug builds require an explicit profile; release
builds use the existing profile with upgrade recovery snapshots. Automatic updates
remain gated while cross-platform acceptance is incomplete. Use the pinned
[released v0.3.0 desktop baseline](resources/test-fixtures/legacy-desktop-release.json)
for comparisons. The React desktop source and browser harness have been retired;
Git protocol oracles and native fixture generators remain isolated test tools.
macOS and Windows release acceptance remains in progress.

## License

[MIT](LICENSE) © 2026 Ruiyang Ke (ruiyangke).
