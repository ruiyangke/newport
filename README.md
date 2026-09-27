<p align="center">
  <img src="src-tauri/icons/icon.png" width="88" alt="Newport app icon" />
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

Reach a development server, dashboard, or database through an SSH tunnel. Discover listening services, choose a local port, and keep track of which connections are working. Automatic reconnect helps recover from temporary interruptions.

### Copy here. Paste there.

Copy text or a screenshot on your computer and paste it into a supported remote application, including Codex. It works even when the server has no graphical desktop. Your clipboard is shared when a remote app requests it.

### Sign in with your own browser

When a remote command opens a link, Newport can open it on your computer. Complete browser-based sign-ins without copying URLs between machines. Supported login callbacks are forwarded back to the server; some services still need a manual step.

### See how your server is doing

Check CPU, memory, storage, and network usage at a glance. Explore up to seven days of history to spot busy periods, spikes, and changes over time.

### Work with terminals and files

Open an interactive SSH terminal. Browse remote folders, preview files, and upload or download over SFTP—all from the same server workspace. Use tmux when you want remote work to continue after leaving the terminal.

### Manage services and containers

Find listening ports, inspect systemd services, and read logs. Browse Docker containers and Compose projects, then start, stop, or restart existing containers.

## Get started

1. **[Download the app](https://github.com/ruiyangke/newport/releases)** for your computer.
2. **Add a server.** Enter its SSH host and username, then choose a key file, SSH agent—including 1Password—or password.
3. **Start working.** Open a terminal, browse files, or add your first tunnel from the sidebar.

| Your computer                              | Download                 |
| ------------------------------------------ | ------------------------ |
| Mac with Apple silicon · macOS 14 or later | macOS ARM64 ZIP          |
| Windows PC with an Intel or AMD processor  | Windows x86_64 installer |
| Windows PC with an ARM processor           | Windows ARM64 installer  |

Windows support is experimental, and its installers are not yet Authenticode-signed. Linux is supported as a remote server, not as a desktop app.

**Coming from Porthop?** Newport is its new name. Version 0.2.4 includes migration for your saved servers, credentials, preferences, and integration setup. Keep your existing app data when upgrading. Until 0.2.4 is published, the latest download still uses the Porthop name.

## Set up clipboard and browser sharing

Open **Integration** for a server and enable **Clipboard**, **Browser**, or both. Newport handles the remote setup. Open a new remote shell afterward, or follow the instructions on that page to use your current session.

Keep Newport running while sharing. You can switch either feature off independently, at any time. Clipboard sharing goes from your computer to the server.

Only enable sharing for servers you trust: remote apps can read sensitive content you copy or request browser tabs while these features are on.

[Read the integration guide →](docs/clipboard.md)

## Made for everyday use

- **Stay connected in the background.** Close the window and keep tunnels running, with access from the menu bar or system tray. Quit the app to disconnect.
- **Choose your appearance.** Use a light or dark theme and optionally launch Newport at login.
- **Update when you’re ready.** Check **Settings → Updates**. Updates download automatically and are verified before installation; restart to apply them. Restarting disconnects sessions and cancels transfers.
- **Keep credentials on your computer.** Saved profiles and passwords are encrypted, with the vault key stored in macOS Keychain or Windows Credential Manager.

Metric history is stored separately and is not encrypted. Clear it anytime in **Settings → Cache**. See [storage and recovery](docs/storage.md) and [security notes](docs/security.md) for backup requirements and known limitations.

## Before you connect

Newport uses your SSH account’s permissions. Monitoring needs Linux utilities, service management needs systemd, and container features need Docker access. Most features need no remote installation; clipboard and browser sharing use an agent managed by Newport.

SSH configuration aliases, ProxyJump, host certificates, and interactive MFA are not currently supported. Container actions manage existing containers; they do not deploy or recreate Compose projects.

## Help shape Newport

Found a bug or have an idea? [Open an issue](https://github.com/ruiyangke/newport/issues). For bugs, include your app version, operating system, and steps to reproduce, with private information removed from any logs.

Contributions are welcome. The [testing guide](docs/testing.md) covers local checks and tests against disposable Linux servers.

## License

[MIT](LICENSE) © 2026 Ruiyang Ke (ruiyangke).
