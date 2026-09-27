# Newport

**Your remote servers, in one desktop app.**

Newport brings SSH tunnels, terminals, files, and server monitoring to macOS and Windows. Connect to a Linux host, see what is running, and work with it from your desktop.

[Download](https://github.com/ruiyangke/newport/releases) · [User guides](docs/README.md) · [Report an issue](https://github.com/ruiyangke/newport/issues)

## One workspace per server

- **Connections** — forward local ports to remote services, check destination health, and reconnect after temporary interruptions.
- **Overview** — monitor CPU, memory, disks, and network traffic, with up to seven days of history.
- **Services** — discover listening ports and inspect systemd services and logs.
- **Containers** — browse Docker containers and Compose projects, view logs, and start, stop, or restart existing containers.
- **Commands** — use an interactive SSH terminal.
- **Files** — browse, preview, upload, and download over SFTP.
- **Integration** — paste local text and images into remote applications and open remote web links in your local browser.

Newport runs as a regular desktop app with a menu-bar or system-tray entry. Closing the window keeps connections running; quitting disconnects them.

## Install

Choose the package for your computer from [GitHub Releases](https://github.com/ruiyangke/newport/releases).

| Platform                          | Package                  |
| --------------------------------- | ------------------------ |
| macOS 14 or later · Apple silicon | macOS ARM64 ZIP          |
| Windows · Intel or AMD            | Windows x86_64 installer |
| Windows · ARM                     | Windows ARM64 installer  |

Windows support is experimental. Windows installers are not Authenticode-signed; updater signatures verify updates separately. Linux is supported as a remote host, not as a desktop platform.

The latest published release may still carry the **Porthop** name while Newport 0.2.4 is in preparation.

## Connect your first server

1. Open Newport and choose **Add Server**.
2. Enter the SSH host and username. Authenticate with a key file, an SSH agent (including 1Password), or a password.
3. Choose a workspace from the sidebar. Add a tunnel in **Connections**, or open a shell in **Commands**.

Most features work through SSH without installing software on the server. Clipboard and browser integration use a small agent that Newport installs and manages when enabled.

## Bring your desktop into remote sessions

Enable **Clipboard** or **Browser** on the server’s **Integration** page. They can be used independently.

Clipboard content is fetched on demand when a remote application requests it. The agent supports text and images through clipboard-only X11 and Wayland displays, including on headless servers. Compatible command aliases also let you read text directly:

```sh
xclip -selection clipboard -o
```

Browser integration opens links requested by remote commands in your local browser. Supported loopback login callbacks use temporary SSH forwarding; not every login flow can be detected automatically.

Shell setup is guarded so a missing agent does not break startup. Open a new shell after setup, or follow the instructions in Integration for an existing session.

**Enable integration only for servers you trust.** Remote applications can request sensitive clipboard contents or open browser tabs while the corresponding integration is enabled.

See the [integration guide](docs/clipboard.md) for setup, supported clients, and troubleshooting.

## Updates and the Porthop rename

Check **Settings → Updates** for new versions. Installed builds check automatically and download verified updates in the background. Restart when ready; restarting disconnects sessions and cancels active transfers.

Newport is the new name for Porthop. Version 0.2.4 includes migration for existing profiles, credentials, preferences, and agent configuration. Keep your existing app data when upgrading. Builds with the updater can receive Newport through Settings once the release is published; older builds need a manual installation.

## Your data

Server profiles and saved passwords are encrypted locally. The vault key lives in macOS Keychain or Windows Credential Manager. Metric history is stored separately, is not encrypted, and can be cleared in **Settings → Cache**.

History keeps 10-second readings for the most recent 24 hours and one-minute summaries for the remainder of seven days. Older readings are removed automatically.

SSH host keys are trusted on first use; changed or revoked keys are rejected. For backup requirements and known security limitations, read [storage and recovery](docs/storage.md) and [security](docs/security.md).

## Supported workflows

- Monitoring requires Linux utilities; service inspection requires systemd; container features require Docker access under your SSH account.
- SSH configuration aliases, ProxyJump, host certificates, and interactive MFA are not supported.
- Leaving Commands closes its terminal session. Use tmux or another session manager for persistent remote work.
- Container actions manage existing containers. Newport does not deploy Compose files or recreate projects.
- Clipboard sharing goes from your desktop to the server; it is not bidirectional.

## Contribute

Bug reports and pull requests are welcome. Include your app version, operating system, and steps to reproduce. Remove credentials and private clipboard contents from logs before sharing them.

Newport uses React, TypeScript, Tauri, and Rust. See the [testing guide](docs/testing.md) for local checks and the Testcontainers suite, which exercises the backend against disposable Linux SSH servers.

## License

[MIT](LICENSE) © 2026 Ruiyang Ke (ruiyangke).
