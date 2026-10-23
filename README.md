<p align="center">
  <img src="resources/icons/icon.png" width="88" alt="Newport app icon" />
</p>

<h1 align="center">Newport</h1>

<p align="center"><strong>Your remote server, in one workspace.</strong></p>

<p align="center">
  Connect to your Linux servers from macOS or Windows.<br />
  Manage tunnels, terminals, files, Git projects, services, and containers—without leaving the app.
</p>

<p align="center">
  <a href="https://newport.app/#download"><strong>Download Newport</strong></a> ·
  <a href="docs/README.md">User guides</a> ·
  <a href="https://github.com/ruiyangke/newport/issues">Report an issue</a>
</p>

[Features](#features) · [Installation](#installation) · [Development](#development)

## Features

Per-server workspaces with independent connection settings and integration permissions. Light and dark themes.

### Connections

- Automatic port discovery, grouped by project, Git worktree, or Compose project.
- Local and reverse SSH forwarding, individually or by project.
- Background tunnels with automatic reconnection. Closing the window preserves connections; quitting disconnects them.

### Terminal

- Tabbed SSH terminals with custom names and output search.
- Automatic reconnection after interruptions. Reconnection starts a fresh shell; persistent remote sessions require tmux or an equivalent tool.

### Files

- SFTP browsing, path navigation, filtering, and hidden files.
- Source previews with syntax highlighting and search; image previews with fit and zoom; PDF previews with page navigation and selectable text.
- Uploads and downloads with progress and cancellation. Unsupported binary formats remain download-only.

See [supported formats and preview limits](docs/files.md).

### Projects and Git

- Remote repository library, branch switching, and worktree creation and management.
- Working-tree diffs, staging, unstaging, and commits.
- Commit history and per-commit file diffs.
- Fetch, pull, push, remotes, tags, and stashes.

### Monitoring

- Live CPU, memory, load, network, storage, and process data.
- Resource history from five minutes to seven days, with sampled charts for longer ranges.

### Services

- systemd service status, failed-service visibility, and unit inspection.
- Live logs with search; start, stop, and restart actions.

### Containers and Compose

- Container status, resource usage, environment, mounts, and networks.
- Live logs, container shells, lifecycle actions, and published-port forwarding.
- Compose project and service actions, including creating missing containers and recreating changed services.
- Compose configuration and activity; disk usage for images, volumes, and build cache.

### Desktop integrations

- **Clipboard:** on-demand text and image access from supported remote tools, including on headless servers.
- **Browser:** remote links open locally, with forwarding for supported login callbacks.
- **Shell integration:** terminal commands and shell setup, managed alongside agent status in **Integrations**.

Newport must be running for desktop integrations. Some authentication flows require a manual callback step.

## Installation

Download the latest release from [newport.app](https://newport.app/#download).
See [release notes](https://newport.app/releases) for changes.

| Platform | Package |
| --- | --- |
| macOS 14+, Apple silicon | [Download ZIP](https://newport.app/downloads/latest/macos-arm64) |
| Windows x86_64 | [Download installer](https://newport.app/downloads/latest/windows-x86_64) |
| Windows ARM64 | [Download installer](https://newport.app/downloads/latest/windows-aarch64) |

Windows support is experimental; installers are not Authenticode-signed. Remote hosts run Linux; there is no Linux desktop build.


## Development

Newport is built with Rust and GPUI. See the [development guide](crates/newport-app/README.md) for builds and packaging, and the [testing guide](docs/testing.md) for local and remote checks.

Set the repository's Git hooks once, and keep them on: `git config core.hooksPath .githooks`. They keep commit messages under 150 characters and keep the private newport-remote crates out of every Cargo manifest, lockfile and Cargo configuration; CI runs the same check.

[User documentation](docs/README.md) · [Issues and feature requests](https://github.com/ruiyangke/newport/issues)

Bug reports should include app and OS versions, reproduction steps, and relevant logs with sensitive information removed.

## License

[MIT](LICENSE) © 2026 Ruiyang Ke (ruiyangke).
