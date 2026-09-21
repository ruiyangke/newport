# Porthop

A macOS workspace for your remote servers. Manage SSH tunnels, monitor Linux hosts, open terminals, and transfer files from one app.

## What you can do

- **Connect:** save servers, forward local ports, and discover listening services.
- **Monitor:** inspect CPU, memory, storage, network activity, and recent usage history.
- **Manage containers:** browse Docker containers and Compose projects, read logs, and start, stop, or restart existing project containers.
- **Inspect services:** browse systemd services and their logs.
- **Work remotely:** open an interactive SSH terminal and browse, preview, upload, or download files over SFTP.
- **Share your clipboard:** send clipboard changes from your Mac to a trusted server, with X11, Wayland, and headless support.

Porthop works as a regular Mac app, with a Dock icon, a menu-bar entry, light and dark themes, and optional launch at login. Closing the window keeps connections running; quitting disconnects them.

## Get started

Porthop requires **macOS 14 or later**. The project currently requires a local build, including a signed, provisioned app bundle for Keychain access. Distribution signing and notarization are not configured yet.

Once running:

1. Choose **Add Server** and enter its SSH connection details.
2. Select a key file, SSH agent, 1Password agent, or password authentication.
3. Open a workspace from the sidebar. Use **Connections** to add tunnels and **Commands** to connect a terminal.

Press **⌘K** to find a server or switch workspaces. Open **Settings** to change appearance or launch-at-login behavior.

## Clipboard sync

Enable **Clipboard sync** in Connections to share your Mac clipboard with that server. Sharing is one-way, remembers your choice, and automatically reconnects after temporary connection failures.

On the server, read text with:

```sh
xclip -selection clipboard -o
```

Porthop can publish through native `wl-copy` or `xclip`. When no desktop clipboard is available, it installs a read-only `xclip` shim and shows PATH setup instructions if needed. Headless servers do not need an X display.

Only enable sharing for servers you trust: copied passwords and other sensitive content are included, and remote clipboard files may remain after a lost connection. See [clipboard setup and behavior](docs/clipboard.md).

## Compatibility and limits

- Monitoring requires Linux and standard system utilities. Service inspection requires systemd; container features require Docker access. Monitoring does not install agents. Port discovery may use existing non-interactive sudo permission to identify processes.
- SSH supports key files, system and 1Password agents, and passwords. SSH configuration aliases, ProxyJump, host certificates, and interactive MFA are not supported.
- Leaving the terminal workspace closes its SSH session. Detached remote jobs may continue; use a session manager such as tmux when you need persistence.
- Container actions operate on existing containers. They do not deploy Compose files or recreate projects.
- Linux and Windows desktop builds are not supported. Distribution signing and notarization are not configured yet.

## Data and security

Server profiles and saved SSH passwords are encrypted locally, with the vault key stored in the macOS Data Protection Keychain. Metric history is stored separately and is not encrypted. A profile backup requires its original Keychain key to restore.

SSH host keys are recorded on first use; changed or revoked keys are rejected. Clipboard sharing is opt-in for each server.

**Known limitation:** the SSH dependency tree includes an RSA implementation affected by a timing side-channel advisory. RSA compatibility remains enabled. Read the [security notes](docs/security.md) before relying on the app for sensitive workflows.

See [storage and recovery](docs/storage.md) for data locations and backup limits.

## Documentation

Browse the [user guides](docs/README.md) for clipboard setup, file transfers, storage, and security information.

## License

[MIT](LICENSE) © 2026 Ruiyang Ke (ruiyangke). Third-party dependencies and bundled fonts retain their respective licenses.

## Updates

Installed direct-download builds check for updates on launch and every six hours.
Updates download in the background and are verified before installation. Open
**Settings → Updates** to check manually or choose **Restart to update**.
Restarting disconnects sessions and cancels active transfers; saved servers and
settings are preserved.

Older builds without the updater need one manual upgrade. Development builds do
not update themselves. Updates become available when a signed release and its
update manifest are published.
