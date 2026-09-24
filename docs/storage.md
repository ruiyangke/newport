# Storage and recovery

Porthop saves its data in `~/Library/Application Support/Porthop` on macOS and `%LOCALAPPDATA%\Porthop` on Windows.

## What is saved

| Data | Location and protection |
| --- | --- |
| Servers, tunnels, SSH passwords and integration preferences | `profiles.stronghold`, encrypted together |
| Vault unlock key | macOS Data Protection Keychain or Windows Credential Manager |
| Appearance and sidebar preferences | `preferences.json` |
| Metric history | `metrics.sqlite3`, not encrypted; endpoint identifiers use keyed fingerprints |
| SSH private keys | Your existing files or SSH agent; agent private keys are not imported |

The vault key is scoped to the profile directory. On macOS, access also requires the signed app’s Keychain access group. On Windows, the key belongs to the current user on that machine. Keys do not roam or sync through iCloud. Saved passwords and the vault key are not sent to the web interface.

Metrics keep 10-second readings for the latest 24 hours and one-minute summaries for the rest of the seven-day retention period. Background cleanup runs on startup and every 15 minutes.

## Clear metric history

Open **Settings → Cache** to see metrics storage size and clear recorded history for all servers. Sampling continues, and saved servers, passwords and settings are preserved. The size includes database journal files; an empty database still uses a small amount of space.

## Backups

A profile backup requires both the encrypted snapshot and its original vault key. Copying `profiles.stronghold` alone is insufficient. Porthop has no portable export or key-recovery interface.

A missing key or damaged snapshot blocks edits instead of replacing existing data. Unsupported profile versions also block loading. Preserve the files and original Keychain or Credential Manager entries when troubleshooting; do not delete them to dismiss an error.

Older installations may retain a login-Keychain key after transfer to the Data Protection Keychain. That item is a recovery copy; ordinary launches use the transferred key.

## Saved connection preferences

Clipboard and Browser preferences are remembered separately for each server. Temporary connection failures do not clear them. Turn either switch off in **Integration** to disable it. Changing a server's host, port or username clears both permissions for the new destination.

Renaming a server preserves active sessions. Connection and authentication changes restart affected sessions. Deleting a server removes its saved profile and passwords, stops its sessions and requests metric-history cleanup. Cleanup errors are reported separately.

## Protection limits

Profile saves are verified before atomic replacement. The encrypted vault protects saved profiles, not everything on your computer or server. Metric measurements, external SSH files, old backups and legacy `SSHTunnelBar` files remain outside the vault.

Remote clipboard files may remain after a lost connection. Database migrations cannot erase filesystem snapshots or old backups. See [clipboard cleanup](clipboard.md#reconnection-and-cleanup) and [security notes](security.md).
