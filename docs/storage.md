# Storage and recovery

Porthop saves its data in `~/Library/Application Support/Porthop`.

## What is saved

| Data | Location and protection |
| --- | --- |
| Servers, tunnels, SSH passwords and clipboard preferences | `profiles.stronghold`, encrypted together |
| Vault unlock key | Device-local macOS Data Protection Keychain |
| Appearance and sidebar preferences | `preferences.json` |
| Metric history | `metrics.sqlite3`, not encrypted; endpoint identifiers use keyed fingerprints |
| SSH private keys | Your existing files or SSH agent; agent private keys are not imported |

The vault key is scoped to the profile directory and signed app's Keychain access group. It is not synced through iCloud. Saved passwords and the vault key are not sent to the web interface.

## Clear metric history

Open **Settings → Cache** to see metrics storage size and clear recorded history for all servers. Sampling continues, and saved servers, passwords and settings are preserved. The size includes database journal files; an empty database still uses a small amount of space.

## Backups

A profile backup requires both the encrypted snapshot and its original Keychain key. Copying `profiles.stronghold` alone is insufficient. Porthop has no portable export or key-recovery interface.

A missing key or damaged snapshot blocks edits instead of replacing existing data. Unsupported profile versions also block loading. Preserve the files and original Keychain items when troubleshooting; do not delete them to dismiss an error.

Older installations may retain a login-Keychain key after transfer to the Data Protection Keychain. That item is a recovery copy; ordinary launches use the transferred key.

## Saved connection preferences

Clipboard sharing is remembered per server. Temporary connection failures do not clear that preference, and Porthop retries automatically. **Disable sync** saves the off state. Changing a server's host, port or username clears clipboard consent for the new destination.

Renaming a server preserves active sessions. Connection and authentication changes restart affected sessions. Deleting a server removes its saved profile and passwords, stops its sessions and requests metric-history cleanup. Cleanup errors are reported separately.

## Protection limits

Profile saves are verified before atomic replacement. Stronghold protects saved profiles, not everything on the Mac or server. Metric measurements, external SSH files, old backups and legacy `SSHTunnelBar` files remain outside the vault.

Remote clipboard files may remain after a lost connection. Database migrations cannot erase filesystem snapshots or old backups. See [clipboard cleanup](clipboard.md#reconnection-and-cleanup) and [security notes](security.md).
