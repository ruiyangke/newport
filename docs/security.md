---
title: "Security and privacy"
description: "Understand SSH trust, saved credentials, sharing permissions, and known security limitations."
slug: security-and-privacy
---

# Security and privacy

Newport connects to servers using your SSH account. The actions you take in terminals, files, Git, services, and containers operate with that account's permissions.

## Trusting an SSH server

Newport checks SSH host keys and trusts an unknown key on first use. A changed or revoked key is rejected. A changed key can mean the server was rebuilt, but it can also mean you reached a different machine.

Newport reads your computer's `~/.ssh/known_hosts` file (Windows: `%USERPROFILE%\.ssh\known_hosts`) and the system SSH known-hosts file, when present. SSH host certificates are not currently supported.

### Resolve a changed host key

Only update a saved key after the server administrator confirms the replacement fingerprint through a trusted channel. A successful connection alone does not verify a replacement key.

1. Get the expected new fingerprint from the administrator. Confirm the host address and SSH port too.
2. On **your computer**, make a backup copy of your `known_hosts` file with Finder or File Explorer. Do not replace the whole file.
3. If the changed entry is in your own file, remove only that host's old entry using the appropriate command below. Replace `server.example.com` with the exact SSH host saved in Newport.

   macOS, in Terminal:

   ```sh
   ssh-keygen -R server.example.com -f "$HOME/.ssh/known_hosts"
   ```

   Windows, in PowerShell:

   ```powershell
   ssh-keygen -R server.example.com -f "$env:USERPROFILE\.ssh\known_hosts"
   ```

   For a nonstandard SSH port, use a quoted host-and-port value, such as `"[server.example.com]:2222"`, in place of the hostname.

4. Before retrying Newport, connect from that same local terminal with `ssh user@server.example.com` (add `-p 2222` for a custom port). Compare the fingerprint shown by SSH with the administrator's value. Accept it only if they match; otherwise stop and contact the administrator.
5. Retry the connection in Newport.

A **revoked** key needs administrator review, not removal of its revocation. If the error comes from the system file (`/etc/ssh/ssh_known_hosts` on macOS or `%ProgramData%\ssh\ssh_known_hosts` on Windows), ask the administrator to correct that file. Do not delete a system or `@revoked` entry to make the connection succeed.

## Saved credentials and local data

Saved server profiles and SSH passwords are encrypted. Their unlock key stays in macOS Keychain or Windows Credential Manager. Newport uses your existing SSH key files or SSH agent; it does not import private keys held by an agent.

Metric history is not encrypted. Downloaded files and files you save from the clipboard are also outside Newport's encrypted profile. See [Storage and recovery](storage.md) for data locations and backup limits.

## Clipboard and browser permissions

Clipboard and Browser permissions are off until you enable them for a server. They are independent, and you can revoke either in **Integrations**.

With Clipboard enabled, programs running under your remote account can request copied text and images, including sensitive content such as passwords. Requested data is cached in memory until the clipboard changes or sharing disconnects. Turning sharing off cannot erase copies a remote application has already saved.

With Browser enabled, remote programs can ask Newport to open HTTP or HTTPS links on your computer. Opening a link does not confirm that the site is trustworthy or that a login completed.

Terminal text copying is separate from clipboard sharing. Newport accepts plain-text copies from programs using OSC 52, but those terminal requests cannot read or clear your local clipboard. See [Clipboard and browser sharing](clipboard.md).

## Port forwarding

Local forwards listen only on your computer's loopback interface, so other devices cannot directly use those local ports.

For reverse forwards, use `127.0.0.1` as the remote bind address to keep the port private to the server. A different bind address may expose it more widely if the SSH server allows it. Check the address before starting a reverse forward. See [Connections](connections.md).

## Installing the remote helper

Newport asks before installing Needle on a server. After installation is approved, it can update the helper while connected. Installing the helper does not enable clipboard or browser sharing by itself.

Port discovery may use existing passwordless sudo access to identify services owned by other accounts. Other administrative actions still depend on your server permissions. Review commands and prompts when managing services, containers, or Remote apps dependencies.

## Known RSA limitation

Newport retains RSA SSH-key support. Its SSH dependencies include an RSA implementation covered by [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html), a timing vulnerability that can expose private-key information. As checked on 25 October 2026, the advisory lists no patched version.

A source review did not establish that Newport is unaffected. RSA compatibility was retained with this known limitation. If you use RSA keys, account for it when deciding how to authenticate; use a non-RSA key where your server and account policy allow it.

## Reporting a concern

For a general problem, follow the [reporting guide](troubleshooting.md#report-a-problem). Do not include passwords, private keys, access tokens, clipboard contents, or an unredacted profile in a public report.

[Back to the user guide](README.md)
