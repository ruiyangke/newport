---
title: "Storage and recovery"
description: "Find where Newport keeps your data, clear recorded history, and keep your data safe when something goes wrong."
slug: storage-and-recovery
---

# Storage and recovery

Newport keeps your servers, passwords and settings on your computer. Files on a server stay on the server unless you download them.

## Find your data

Newport's data folder is `~/Library/Application Support/Newport` on macOS (in Finder, choose **Go** > **Go to Folder**), and `%LOCALAPPDATA%\Newport` on Windows (paste it into File Explorer's address bar).

| What | Where |
| --- | --- |
| Servers, forwards and sharing permissions | `profiles.json`, readable only by your user account |
| SSH passwords | `profiles.json`, each encrypted |
| Newport account sign-in and license | `account.json`, encrypted; removed when you sign out |
| The key that encrypts passwords and the sign-in | Keychain on macOS, Credential Manager on Windows; made when you first save a password or sign in |
| The profile's ID, which names that key | `profile-id` |
| Preferences, such as theme, sidebar and Remote apps settings | `preferences.json` |
| Window size and position, and where you left off | `ui-state.json` |
| Resource history | `metrics.sqlite3`, not encrypted |
| SSH private keys | Not stored: Newport uses your key files or SSH agent |

The key stays on this computer, for this user account. It's named by the ID in `profile-id`, so it follows the data folder if you move it. It doesn't sync through iCloud.

## On the server

Newport's helper, Needle, is installed in `~/.local/share/newport` on the server. [What runs on your server](security.md#what-runs-on-your-server) lists what else it adds.

Shells that keep running after you quit Newport hold their screen and scrollback in memory, not on disk. For each shell, Needle keeps a small private file with its name, size and the names of the computers using it, in `newport-ipc/terminals` under your runtime folder (usually `/run/user/<your user id>`), or in `~/.cache/newport/run/terminals`.

In **Integrations**, open **More Needle actions**, then choose **Uninstall…**. Confirming removes everything Needle keeps and ends the shells it kept running.

## Clear resource history

See [Clear the history](monitoring.md#clear-the-history) in Server monitoring.

## Back up your data

Saved passwords and the sign-in open only with this computer's key, which stays behind when you copy the data folder. Newport has no export.

- Before you replace your computer or reinstall its system, note each server's address, username, sign-in method and forwards somewhere safe, and keep your passwords and SSH keys. You'll add the servers again; your Newport account doesn't bring them back.
- For a safety copy on this computer, quit Newport and copy the whole data folder to a dated folder on a backup drive. Leave the original in place.

Back up your server files and SSH keys separately; the data folder doesn't include them.

### Snapshots before an update

When a new version of Newport first starts, it copies your data to `upgrade-backups/native-<version>/data` in the data folder. It keeps the snapshots for the current and previous versions. Snapshots include your saved passwords, still encrypted, but leave out your account sign-in, so after restoring one you sign in again. They also leave out the server metrics history, so the charts start empty after a restore.

If Newport can't make or check a snapshot, it stops with an error instead of opening your data.

## Recover from a startup error

Don't delete files in the data folder, or the key in Keychain or Credential Manager, to clear an error. Newport stops instead of replacing data it can't open, so your data is still there.

1. Quit Newport. If the error says another Newport owns the profile, quit that one too.
2. Copy the whole data folder, including `upgrade-backups`, to a separate dated folder. Keep the original where it is, and keep using the same user account.
3. Note the error and your Newport version, and the version you updated from if this followed an update.
4. [Report the problem](troubleshooting.md#report-a-problem) before you change any files. Work only on copies of a snapshot.

The key's entry is named `app.newport.profile-vault` in Keychain and starts with `app.newport/vault` in Credential Manager, followed by the ID in `profile-id`.

If Newport says the key is missing, or "Cannot unlock saved profiles", it can't read that entry. Unlock Keychain or restore the entry, then start Newport again. Newport never makes a new key while it has passwords or a sign-in the old one encrypted, so nothing changes until it can.

If Newport says "Saved profiles are from a newer Newport. Update Newport before editing them.", your data was saved by a newer version. Install the current version; an older one doesn't roll your data back.

## Change or delete a saved server

1. Select the server and choose **Server** > **Edit server…**.
2. Change **Name (optional)** or the connection fields, then choose **Save server**. To remove the server, choose **Delete server…** and confirm.

- Renaming a server keeps its sessions.
- Changing the **SSH host**, **SSH port**, **Username**, **SSH key**, sign-in method or **Connect through**, or entering a new password, reconnects the server. It also turns off **Clipboard access** and **Browser links**, asks again before installing the helper, and restarts the server's active forwards.
- Deleting a server removes its saved password, its forwards and its recorded history. It doesn't touch the server's files. You can't delete a server that another server uses as its jump host.

See [Security and privacy](security.md) for how saved passwords are protected.

[Back to the user guide](README.md)
