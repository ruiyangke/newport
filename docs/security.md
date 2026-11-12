---
title: "Security and privacy"
description: "How Newport checks your servers, protects saved passwords, shares your clipboard and browser, and what it runs on your server."
slug: security-and-privacy
---

# Security and privacy

Newport signs in to your servers with your SSH account. Everything you do on a server in Newport runs with that account's permissions.

## Trusting an SSH server

The first time you connect to a server, Newport saves its SSH host key to `~/.ssh/known_hosts` (`%USERPROFILE%\.ssh\known_hosts` on Windows). After that, it refuses a server that shows a different key or a revoked one. It also reads the system file, `/etc/ssh/ssh_known_hosts` (`%ProgramData%\ssh\ssh_known_hosts` on Windows). SSH host certificates aren't supported.

A changed key can mean the server was reinstalled, or that you reached a different machine.

### Resolve a changed host key

When Newport says the server's SSH host key changed, replace the saved key only after the server's administrator gives you the new fingerprint through a channel you trust.

1. On your computer, make a backup copy of your `known_hosts` file.
2. Remove the server's old key, using the host as saved in Newport in place of `server.example.com`.

   On macOS, in Terminal:

   ```sh
   ssh-keygen -R server.example.com -f "$HOME/.ssh/known_hosts"
   ```

   On Windows, in PowerShell:

   ```powershell
   ssh-keygen -R server.example.com -f "$env:USERPROFILE\.ssh\known_hosts"
   ```

   For an SSH port other than 22, use `"[server.example.com]:2222"` as the host.

3. In the same terminal, run `ssh user@server.example.com`, adding `-p 2222` for another port. Accept the key only if its fingerprint matches the administrator's.
4. Connect again in Newport.

If the key is revoked, or the old key is in the system file, ask the administrator to fix it. Don't delete those entries to get connected.

## Saved passwords and data

Your SSH passwords and Newport account sign-in are encrypted, each on its own. The key that unlocks them is in Keychain on macOS or Credential Manager on Windows, and doesn't sync to other devices. Your saved servers and forwards are not encrypted; they're in a file only your user account can read. Newport uses your SSH key files or agent and doesn't copy keys out of the agent.

Resource history is an ordinary file too, and so are files you download or paste from the server. See [Storage and recovery](storage.md).

## Clipboard and browser permissions

**Clipboard access** and **Browser links** are both on in the setup step until you turn them off, and off for a server saved any other way. You can turn either on or off at any time in **Integrations**.

- With **Clipboard access** on, programs running under your account on the server can read what you copy, including text and images such as passwords. Turning it off doesn't erase copies a program already saved.
- With **Browser links** on, programs on the server can open `http` and `https` links in your browser. Opening a link doesn't mean the site is safe.

Programs in a terminal can always copy text to your clipboard. Only **Clipboard access** lets them read it. See [Clipboard and browser sharing](clipboard.md).

## Port forwarding

Forwards to the server listen only on your own computer, at `127.0.0.1`. Other devices on your network can't use them.

For a forward from the server to your computer, keep **Listen address on the server** at `127.0.0.1`. Another address can let other machines reach it. See [Connections](connections.md).

## What runs on your server

Newport asks before it installs its helper, Needle, on a server, and then keeps it up to date. Needle is installed in your home directory and runs as your account. Installing it doesn't turn on sharing.

When you finish setup with either sharing switch on, or later turn one on in **Integrations**, Needle also adds the links `xdg-open` and `newport-browser` in `~/.local/bin`, and a marked block to your shell's startup files, such as `~/.bashrc` or `~/.zshrc`.

Needle uses `sudo` only where your account already has it without a password, to see other accounts' ports and to change system services. It never asks for a password. When a service change needs one, Newport puts the command in a terminal for you to run.

[Remote apps](remote-apps.md) installs Newport Remote on the server, and may offer commands that install system packages. Read each command before you run it.

## RSA keys

Newport supports RSA keys, but the RSA code it uses is affected by [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html), a timing weakness that can reveal information about a private key. There is no fixed version yet, and Newport hasn't been shown to be unaffected. Use an Ed25519 or ECDSA key where you can.

## Reporting a concern

Follow [Report a problem](troubleshooting.md#report-a-problem). Never include passwords, private keys, tokens, clipboard contents or your data folder in a public report.

[Back to the user guide](README.md)
