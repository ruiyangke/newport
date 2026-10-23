# Release publishing

Newport checks `https://newport.app/updates/latest.json`. Signed packages and
release metadata live in the private `newport-releases` Cloudflare R2 bucket.
The website Worker serves them through `/updates/*`. Desktop distribution and
website deployment are independent: this repository's release workflow does
not check out, build, change or deploy the website.

Packages retain the existing Minisign trust key, signed release version and
filename. macOS updates use the signed `.app.tar.gz`; manual installation uses
the ZIP. Windows uses the same signed EXE for both.

## Build and publish

1. Update the app versions, commit the desktop source, and tag the release.
2. Run the Release workflow for that tag. It builds, signs, notarizes and
   verifies all packages from the exact tagged commit. Its final **Release:
   publish to Cloudflare** job runs `cargo xtask release deploy release`
   (using the precompiled xtask binary when credentials are present).
3. The publisher verifies all three update targets, signed versions and
   filenames, the macOS installer, and the complete `SHA256SUMS`. It uploads
   immutable files to R2 and reads them back before updating the latest feed.
   It does not create a GitHub release.

CI retains `release-<tag>` (installers, update payloads, signatures,
`latest.json`, `SHA256SUMS`) for 90 days. R2 objects remain available
independently of this retention period. Release notes displayed on the website
and its manual-download catalog are maintained by the website's separate
publishing process; they are not prerequisites for a desktop release.

## CI credentials

In Cloudflare's R2 dashboard, create an R2 API token with **Object Read & Write**
permission limited to the **newport-releases** bucket. Save its S3 credentials
in the repository's GitHub Actions secrets:

- `R2_ACCESS_KEY_ID`
- `R2_SECRET_ACCESS_KEY`

Set repository variable `CLOUDFLARE_ACCOUNT_ID` to
`94e989bb33d395c6b3c0e6ff8eb84184` (RYKE). The publisher authenticates directly
to the account's R2 S3 endpoint. It does not need `CLOUDFLARE_API_TOKEN`,
Workers deployment permissions, D1 permissions, Wrangler, or Node.
See [Cloudflare's R2 token guide](https://developers.cloudflare.com/r2/api/tokens/).

The workflow checks that credentials are configured before platform builds.
Only that presence check and the final upload step receive storage secrets;
Rust compilation runs beforehand. Runs are serialized, never cancelled during
publication, and recorded under the `cloudflare-production` GitHub environment.

## Local publication and recovery

Use an extracted `release-<tag>` workflow artifact, or a complete distribution
produced by `cargo xtask release merge`. Verify it without credentials or
network access:

```sh
cargo xtask release deploy /absolute/path/to/release --dry-run
```

To publish, provide `CLOUDFLARE_ACCOUNT_ID`, `R2_ACCESS_KEY_ID` and
`R2_SECRET_ACCESS_KEY` in the environment, then run:

```sh
cargo xtask release deploy /absolute/path/to/release
```

`--bucket` overrides the default `newport-releases` bucket. Uploads are limited
to the expected distribution files; unrelated files in the directory are
excluded. The publisher refuses older versions and replacement of an existing
version with different metadata or bytes. An identical retry resumes a partial
upload without rewriting existing files. If the active feed changes during an
upload, its conditional update fails rather than replacing the other release.
Inspect the active version before retrying such a conflict.

Run the [signed upgrade acceptance procedure](testing.md#upgrade-acceptance)
before shipping a new app version. The feed must never advertise unverified
builds.

## Storage contract

The independently deployed website Worker reads these objects at request time:

- `updates/latest.json`: the active signed update manifest, written last.
- `releases/<version>/release.json`: immutable JSON containing `version`, the
  complete `manifest`, and `artifacts` keyed by filename, each with `size` and
  `sha256`.
- `releases/<version>/<filename>`: the verified distribution files, including
  the version's `latest.json` and `SHA256SUMS`.

`https://newport.app/updates/latest.json` serves the active manifest without
caching. `/updates/<version>/latest.json` serves a retained version's manifest;
`/updates/<version>/<filename>` streams an artifact named in its release record.
Payloads use the website's SHA-256 keyed edge cache and range handling.
Automatic updates do not increment installer download counts or access D1.

The website requires a one-time deployment of this R2 reader. Later desktop
releases only publish R2 objects, and website deployments leave the active
release unchanged. Keep published artifacts. If a release needs to be withdrawn,
an operator can restore a previously verified manifest at `updates/latest.json`;
the normal publisher deliberately refuses downgrades. Installed newer apps do
not downgrade merely because the advertised version is older.

## Transition from GitHub

Existing apps through 0.5.0 have the GitHub feed URL compiled in. Hosting their
unchanged signed packages on the website does not change their configuration.
The first newer release built with the website endpoint must also be announced
at the old GitHub feed, so existing users can install it normally.

After that newer release is live and verified on the website, create/publish a
GitHub transition release with a copy of the **website's live `latest.json`**
attached. Its payload URLs point to Newport's website, so the legacy app obtains
only the manifest from GitHub and downloads its signed update from Newport.
Keep this GitHub release marked latest and retain its website payloads. It stays
available for users who skipped the migration; after installing it, all further
checks use the website. The release workflow does not publish this bridge
automatically. Do not delete the old feed or change the signing key.

Users can also install the newer version directly from the website. Moving
0.5.0's existing payloads to website storage is preparation, not a new desktop
release or a completed migration of installed clients.
