---
title: "Troubleshooting"
description: "Resolve common connection and workspace problems or gather details for a support report."
slug: troubleshooting
---

# Troubleshooting

Start with the error shown in the affected workspace. Confirm that you have selected the intended server, then check its connection and **Integrations** status. The steps below address common problems without discarding saved data.

## Cannot connect over SSH

1. Check the host, SSH port, username, and network or VPN connection.
2. Open **Server → Edit server…** and confirm the authentication method.
3. For agent keys, unlock your SSH agent and choose **Refresh agent keys**. Follow [Prepare your SSH key](getting-started.md#prepare-your-ssh-key) to load and verify a passphrase-protected key on your computer.
4. Use **Server → Test connection** or retry the failed operation.

For a server behind a jump host, Newport signs in to the jump host first, so check the jump host the same way. For a server reached with `ProxyCommand`, Newport shows what the command printed; run the command in a terminal to check it.

For **connection refused** or a timeout, check that SSH is listening on the selected address and allowed through the network. For **authentication failed**, check the user and key/password rather than changing unrelated network settings.

### Host key changed or revoked

Verify the fingerprint with the server administrator before changing your known-hosts entry. Do not delete a key solely to bypass the error. Follow [Resolve a changed host key](security.md#resolve-a-changed-host-key) for file locations and the verified replacement procedure.

## Needle setup or a server feature fails

Open **Integrations** and check Needle's status. Use the available install or update action and read any connection, permission, or installation error. Confirm that the SSH account can write to its home directory and that the server has space for the helper.

If the helper is newer than your desktop app, update Newport first. An interrupted installation can be retried. Uninstalling Needle removes its managed files and stops its services; use it only when you intend to remove the helper.

Plain terminals, manual forwards, and SFTP remain useful while you resolve a helper problem. For a feature-specific issue, use its guide:

| Problem | Guide |
| --- | --- |
| A forward fails or a web app will not load | [Connections](connections.md#troubleshooting) |
| A shell disappeared after reconnecting | [Terminals](terminal.md#keep-work-running-through-a-disconnect) |
| A preview or transfer fails | [Remote files](files.md) |
| A file changed on the server while being edited | [Project file conflicts](projects.md#handle-a-file-changed-elsewhere) |
| Completion, diagnostics, or formatting is missing | [Language tools](language-servers.md#when-a-tool-cannot-finish) |
| Clipboard or browser sharing does not work | [Sharing](clipboard.md#reconnection-and-troubleshooting) |
| A graphical application will not launch | [Remote apps](terminal-gui.md#troubleshooting) |
| Service actions are denied | [Services](services.md#troubleshooting) |
| Docker or Compose is unavailable | [Containers](containers.md#troubleshooting) |

## Newport is still running after closing the window

This is expected. Closing the window keeps background connections running. Reopen Newport from its menu bar or system-tray icon, or choose **Quit Newport** to exit completely.

Before quitting, save editor and remote-app changes and finish transfers. A restart disconnects sessions; reconnecting a terminal starts a new shell.

## An update or profile error prevents startup

Preserve your Newport data folder and its original Keychain or Credential Manager entries. Deleting them can make saved profiles unrecoverable.

If another Newport instance owns the profile, quit it and retry. For damaged data, missing keys, or failed upgrade snapshots, follow [Storage and recovery](storage.md#recover-from-a-startup-or-vault-error). For downloads and installation, see [Updates](releases.md#if-an-update-does-not-work).

## Report a problem

[Open an issue](https://github.com/ruiyangke/newport/issues) with:

- Your Newport version from **Settings → Updates** or **About Newport**.
- Your computer's OS version and processor type; include the server's Linux distribution when relevant.
- The page or feature you were using, the steps that reproduce the problem, and what you expected to happen.
- The exact error text or a redacted screenshot. Use **Copy details** if the error offers it.
- Whether the problem followed an update, reconnect, authentication change, or change on the server.

Remove passwords, private keys, tokens, sensitive hostnames, clipboard contents, and private file contents before sharing diagnostics. Do not attach your encrypted profile or credential-store entries to a public issue.

[Back to the user guide](README.md)
