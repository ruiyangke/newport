# Released profile fixture

`v0.3.0.stronghold` was created on macOS ARM64 on 2026-10-13 through
the **signed Newport v0.3.0 release UI**, then copied after normal app shutdown
and before opening it with native Newport. It contains one disposable server:
`Release baseline fixture`, `fixture@127.0.0.1:1`, automatic SSH public-key
authentication, and no tunnels or saved passwords. No real server or credential
is included. The public fixture key is 32 bytes with value `7`, matching
`Store::for_test`.

- Source: `Newport-0.3.0-macos-arm64.zip`, pinned in
  [legacy-desktop-release.json](../legacy-desktop-release.json).
- Archive SHA-256: `e6dd4d9255ca611ea46b91c83e4f960fda762124b01dae547c54a0855f8d9ec7`.
- Snapshot SHA-256: `7bc8ab0bc20801d4cace9105457fd86802faa397a52e7ed36bbd71563fd3db3b`.
- Original app passed strict deep code-sign verification, Gatekeeper notarized
  Developer ID assessment, and stapled-ticket validation. It was not re-signed.

The native profile regression reads these exact released bytes without a Tauri
dependency, verifies connection fields and that reading does not rewrite the
snapshot, then verifies native edit/reopen behavior.

Historical acceptance before the native schema bump used the same disposable profile sequentially:
create/save in 0.3.0, open/edit/save in native 0.3.1, then reopen in 0.3.0.
The released app displayed the native-edited name and preserved connection and
authentication fields. Native save completed and sidebar collapse/reopen worked.
This is bounded profile compatibility evidence, not production credential-store,
signed installer/update, full feature, or matched-appearance pixel acceptance.

Native saves now use schema 3 because native-only reverse tunnels and setup
consent cannot safely round-trip through 0.3.0. Native still reads the exact
schema-2 fixture without rewriting it; 0.3.0 deliberately rejects a profile after
a native save. The historical reopen result above does not apply to schema 3.
Recovery must preserve subsequent native edits rather than silently restoring an
older snapshot. The executable lifecycle suite also boots the released fixture
and verifies both its unchanged live bytes and its recovery copy.

To regenerate, launch the verified release with explicit `NEWPORT_DATA_DIR` and
`NEWPORT_TEST_VAULT_KEY_FILE` pointing to disposable paths. Use the form values
above, save, quit, then copy `profiles.stronghold` before native access. Never
substitute a snapshot produced by the native writer: that loses the independent
release compatibility check. Stronghold snapshots contain randomness, so update
the digest and provenance deliberately when regenerating.
