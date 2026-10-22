# Storage and recovery

Newport saves its data in `~/Library/Application Support/Newport` on macOS and `%LOCALAPPDATA%\Newport` on Windows.

## What is saved

| Data | Location and protection |
| --- | --- |
| Servers, tunnels, SSH passwords and integration preferences | `profiles.stronghold`, encrypted together |
| Vault unlock key | macOS Data Protection Keychain or Windows Credential Manager |
| Newport account sign-in (device credential and license) | `account.stronghold`, encrypted with the same vault key |
| Appearance and sidebar preferences | `preferences.json` |
| Metric history | `metrics.sqlite3`, not encrypted; endpoint identifiers use keyed fingerprints |
| SSH private keys | Your existing files or SSH agent; agent private keys are not imported |

The vault key is scoped to the profile directory. On macOS, access also requires the signed app’s Keychain access group. On Windows, the key belongs to the current user on that machine. Keys do not roam or sync through iCloud. Saved passwords, the vault key and the account's device credential are not sent to the UI.

The account sign-in has its own snapshot so it never contends with profile edits, and older Newport builds never rewrite it. Signing out removes the file. To sign in, Newport opens the account site in your browser, listens on `127.0.0.1` for its one-time code, and exchanges the code with a PKCE verifier; the license refreshes at launch and once a day. Development builds with `NEWPORT_DATA_DIR` set can point at another account site with `NEWPORT_ACCOUNT_URL` (HTTPS, or HTTP on loopback).

Metrics keep 10-second readings for the latest 24 hours and one-minute summaries for the rest of the seven-day retention period. Background cleanup runs on startup and every 15 minutes.

## Clear metric history

Open **Settings → Cache** to see metrics storage size and clear recorded history for all servers. Sampling continues, and saved servers, passwords and settings are preserved. The size includes database journal files; an empty database still uses a small amount of space.

## Backups

A profile backup requires both the encrypted snapshot and its original vault key. Copying `profiles.stronghold` alone is insufficient. Newport has no portable export or key-recovery interface.

A missing key or damaged snapshot blocks edits instead of replacing existing data. Unsupported profile versions also block loading. Preserve the files and original Keychain or Credential Manager entries when troubleshooting; do not delete them to dismiss an error.

### Upgrade recovery snapshots

Before Newport opens credentials, opens metric history, or starts
workers, it reserves profile ownership and preserves a recovery copy under
`upgrade-backups/native-<version>/data`. It copies existing profile files,
including the encrypted vault, preferences, pending-operation records, unknown
files, and SQLite database/WAL files. Runtime sockets, lock files, older
backup directories, temporary files an interrupted save left behind, and the
Newport account sign-in (`account.stronghold`) are excluded: a device
credential never outlives signing out in an old snapshot, and restoring one
means signing in again. Symlinks and special files are rejected rather than
followed. The copy has private permissions and a SHA-256 manifest; it is
published only after verification and filesystem synchronization.

A failed or damaged backup blocks startup before the profile is opened. Retrying does not
overwrite an existing recovery snapshot. New installations may have an empty
pre-startup snapshot. The snapshot for the running version and the one before
it are kept; older ones, and staging folders a crash left behind, are removed
after a new snapshot is published. They are not portable exports and do not
contain keys from OS credential storage.

Newport never automatically restores an older profile during application
rollback: that could discard edits made after upgrading or rewind pending remote
operations. For recovery, first quit every Newport instance, retain the
current profile and OS key, and work from a separate copy of the snapshot. Do not
open the immutable backup directly in SQLite. Keep the original canonical profile
path when restoring encrypted data, and reconcile pending operations before
resuming server work. Remote files and external SSH keys are outside this backup.

## Saved connection preferences

Clipboard and Browser preferences are remembered separately for each server. Temporary connection failures do not clear them. Turn either switch off in **Integrations** to disable it. Changing a server's host, port or username clears both permissions for the new destination.

Renaming a server preserves active sessions. Connection and authentication changes restart affected sessions. Deleting a server removes its saved profile and passwords, stops its sessions and requests metric-history cleanup. Cleanup errors are reported separately.

## Protection limits

Profile saves are verified before atomic replacement. The encrypted vault protects saved profiles, not everything on your computer or server. Metric measurements, external SSH files and old backups remain outside the vault.

Remote clipboard files may remain after a lost connection. Database migrations cannot erase filesystem snapshots or old backups. See [clipboard cleanup](clipboard.md#reconnection-and-troubleshooting) and [security notes](security.md).

## Profile directory and ownership

`NEWPORT_DATA_DIR` selects an isolated profile. The vault key lives in
`app.newport.profile-vault` on macOS and `app.newport/vault/…` on Windows.
Missing or inaccessible keys never cause a replacement vault to be created.

Each launch acquires the profile-local `newport.lock` and holds it through
service drain, preventing two hosts from writing the profile concurrently.
