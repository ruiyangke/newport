---
title: "Storage and recovery"
description: "Find your saved data, understand backup limits, and preserve files when recovery is needed."
slug: storage-and-recovery
---

# Storage and recovery

Newport saves connection details and preferences on your computer. Files you edit on a server stay on that server unless you download them.

## Find your local data

| Platform | Newport data folder |
| --- | --- |
| macOS | `~/Library/Application Support/Newport` |
| Windows | `%LOCALAPPDATA%\Newport` |

On macOS, use Finder's **Go → Go to Folder** to open the folder. On Windows, paste the path into File Explorer's address bar.

| Data | How it is stored |
| --- | --- |
| Servers, tunnels, SSH passwords, integration preferences | Encrypted in `profiles.stronghold` |
| Newport account sign-in and license | Encrypted in `account.stronghold`; signing out removes it |
| Unlock key for encrypted data | macOS Keychain or Windows Credential Manager |
| Appearance and sidebar preferences | `preferences.json` |
| Resource history | `metrics.sqlite3`, without encryption |
| SSH private keys | Your existing key files or SSH agent |

The unlock key belongs to the original profile location and OS credential store. It does not sync through iCloud. On Windows it belongs to the current user on that machine; on macOS, access also depends on the app's signing identity.

## Clear resource history

Open **Settings → Cache** to see the metrics storage size, then choose **Clear cache** to clear recorded history for all servers. This preserves your saved servers, passwords, and settings. Sampling continues afterward.

Newport retains history for seven days: detailed readings for the most recent 24 hours and one-minute summaries for older data. An empty database still occupies a small amount of disk space.

## Back up your data

**Copying the data folder alone is not a portable backup.** Encrypted profiles also need their original unlock key. Newport does not currently provide a portable export or a key-recovery interface.

For a same-computer safety copy, quit Newport and copy the entire data folder to a dated folder on your backup drive. Keep the original in place. This preserves the files while the original OS account and unlock key remain available; it does not guarantee recovery from a damaged or lost key.

**Before replacing your computer or reinstalling its OS**, record your server addresses, usernames, authentication methods, and forwarding settings separately in a secure place. Keep access to your passwords, SSH key files, and agent. You may need to add the servers again. A folder copy cannot transfer the unlock key, and signing in to your Newport account does not recover it.

Preserve the original OS account and its credential-store entries. Keep backups of remote files and external SSH key files separately; Newport's profile backups do not include them. Save editor changes before quitting, since unsaved editor buffers are not a substitute for a file backup.

### Automatic upgrade snapshots

Before opening your profile for a new app version, Newport creates a recovery snapshot under `upgrade-backups/native-<version>/data` inside the data folder. It keeps the snapshot for the running version and the previous one.

These snapshots include saved profile data, preferences, and metric history. They exclude the account sign-in and the OS-stored unlock key. You will need to sign in again after a recovery that replaces account data.

If a snapshot cannot be safely created or verified, startup stops with an error. Retrying does not overwrite an existing recovery snapshot. A new installation may have an empty snapshot because it has no earlier data.

## Recover from a startup or vault error

1. Quit every Newport instance using the affected profile.
2. Copy the entire current data folder, including `upgrade-backups`, to a separate dated folder. Keep the original path and OS account intact. Do not delete or reset credential-store entries.
3. Record the error and the Newport version. If this followed an update, record the previous version too.
4. [Report the problem](troubleshooting.md#report-a-problem) with those details before replacing encrypted data. Use a separate copy of a snapshot for any recovery work and keep the original untouched.

For support, the macOS credential service is named `app.newport.profile-vault`; Windows uses `app.newport/vault`. Its account identifier depends on the profile path. These names help identify the relevant entry; they are not keys or an export procedure.

Do not delete the encrypted profile or its unlock key to clear an error. A missing key, damaged profile, or unsupported profile version stops loading so Newport does not replace existing data.

Recovery must retain the original profile path and matching key. Restoring a snapshot can also restore records of unfinished remote operations, so check those operations before resuming work. Installing an older app does not automatically roll your profile back; doing so could discard newer edits.

Newport permits only one running instance to own a profile. If an error says the profile is already in use, close the other instance before trying again.

## Change or delete a saved server

Select the server, then choose **Server → Edit server…**, or open the server-name picker and choose its **Edit** entry. Change **Name (optional)** to rename it, or update the connection fields, then choose **Save server**. To remove it, choose **Delete server…** in that same dialog and review the confirmation.

Renaming a server keeps its active sessions. Changing its connection or authentication settings restarts affected sessions. Changing the host, port, or username also resets Clipboard and Browser permissions.

Deleting a server removes its saved profile and password, stops its sessions, and requests removal of its metric history. Any cleanup error is reported separately. It does not serve as a backup of the server's files; use the file and server tools appropriate to the data you want to keep.

See [Security and privacy](security.md) for the limits of encrypted storage.

[Back to the user guide](README.md)
