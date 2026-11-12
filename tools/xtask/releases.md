# Release publishing

Newport checks `https://newport.app/updates/latest.json` for updates and
downloads signed packages from `https://newport.app/updates/`. Every package is
signed with the Minisign key whose public half is in `resources/updater.json`,
and the app verifies the signature, version and filename before installing.
macOS updates use the signed `.app.tar.gz`; manual installation uses the ZIP.
Windows uses the same signed EXE for both.

## Build and publish

Releases are built by the maintainer-only Release workflow, which needs the
release signing keys, the private Remote apps source and the storage
credentials. It builds, signs, notarizes and verifies every package from the
tagged commit, keeps the result as the `release-<tag>` artifact for 90 days,
publishes it, and then creates the GitHub release with the notes from
`docs/releases/<version>.md`.

The publisher verifies all three update targets, signed versions and
filenames, the macOS installer and the complete `SHA256SUMS`. It uploads
immutable files, reads them back and only then advances the update feed. It
refuses older versions, refuses to replace a published version with different
bytes, and resumes an identical partial upload. The feed never advertises an
unverified build.

To check a distribution without credentials or network access:

```sh
cargo xtask release deploy /absolute/path/to/release --dry-run
```

Run the [signed upgrade acceptance procedure](testing.md#upgrade-acceptance)
before shipping a new app version.

## Manual installation

If automatic updates are unavailable, install the current release directly from
[newport.app](https://newport.app/#download). Keep the profile and update signing
identities stable so manual installation preserves the user's configuration.
