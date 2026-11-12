---
title: "Troubleshooting"
description: "Fix common connection, terminal and startup problems, or report a problem."
slug: troubleshooting
---

# Troubleshooting

Start with the message on the page that has the problem, and check that the right server is selected. None of the steps below delete your saved data.

## Cannot connect over SSH

1. Check the host, SSH port, username, and your network or VPN.
2. Choose **Server** > **Edit server…** and check how the server signs in.
3. If you use an SSH agent, unlock it and choose **Refresh agent keys**. To load a key with a passphrase, see [Prepare your SSH key](getting-started.md#prepare-your-ssh-key).
4. Choose **Server** > **Test connection**, or try again.

If the message says the connection was refused or timed out, check that SSH is running on that address and port and that your network allows it. If sign-in failed, check the username and key or password.

For a server behind a jump host, Newport signs in to the jump host first, so check the jump host the same way. For a server reached through a `ProxyCommand`, Newport shows what the command printed; run the command in a terminal on your computer to check it. If your SSH config uses a different proxy command for the host, the message shows it with **Use this command**. To compare by hand, run `ssh -G` followed by the host and look for its `proxycommand` line.

### Host key changed or revoked

Don't delete the saved key just to get past the error. Confirm the new fingerprint with the server's administrator, then follow [Resolve a changed host key](security.md#resolve-a-changed-host-key).

## A server feature fails

Read the message on the page. If it asks you to update Newport, another computer that uses this server runs a newer version; see [Keep every computer on the same version](updates.md#keep-every-computer-on-the-same-version).

For anything else, use the feature's guide:

| Symptom | Fix |
| --- | --- |
| A shell ended after you quit Newport or disconnected | The server may end your programs when you log out; ask its administrator. See [Terminals](terminal.md#troubleshooting). |
| A terminal tab says "This terminal uses an incompatible protocol." | See [Terminals](terminal.md#troubleshooting). |
| A terminal says "Open this terminal in Newport to continue." | See [Terminals](terminal.md#troubleshooting). |
| A Linux app won't open, or its windows disappeared | See [Remote apps](remote-apps.md#troubleshooting). |
| A forward fails or a web app won't load | See [Connections](connections.md#troubleshooting). |
| A preview or transfer fails | See [Remote files](files.md). |
| A file changed on the server while you were editing it | See [Projects and Git](projects.md#handle-a-file-changed-elsewhere). |
| Completion, diagnostics or formatting is missing | See [Language tools](language-servers.md#when-a-tool-cannot-finish). |
| Clipboard or browser sharing doesn't work | See [Clipboard and browser sharing](clipboard.md#troubleshooting). |
| A service action is denied | See [Services](services.md#troubleshooting). |
| Docker or Compose is unavailable | See [Containers and Compose](containers.md#troubleshooting). |

## Newport is still running after closing the window

Closing the window hides it, and your connections keep running. To open it again, choose **Open Newport** from Newport's icon in the menu bar or system tray. To exit, choose **Quit Newport**.

## An update or profile error prevents startup

Keep your Newport data folder and its Keychain or Credential Manager entry. Deleting the folder loses your saved servers, and deleting the entry loses your saved passwords and sign-in, for good.

If the error says another Newport owns the profile, quit that one and try again. For other errors, follow [Recover from a startup error](storage.md#recover-from-a-startup-error). For download and install problems, see [If an update does not work](updates.md#if-an-update-does-not-work).

## Report a problem

[Open an issue](https://github.com/ruiyangke/newport/issues) with:

- your Newport version, from **Settings** > **Updates** or **About Newport**;
- your computer's system version and processor, and the server's Linux distribution if it matters;
- what you were doing, the steps that cause the problem, and what you expected;
- the exact message, or a screenshot with private details hidden. Use **Copy details** if the message offers it;
- whether it started after an update, a reconnect, a sign-in change or a change on the server.

Remove passwords, private keys, tokens, private hostnames, clipboard contents and private file contents first. Never attach your data folder or credential entries to a public issue.

[Back to the user guide](README.md)
