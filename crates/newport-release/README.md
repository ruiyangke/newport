# Release verification and signing

The default library verifies Needle release identity. The optional `signing`
feature enables the offline release CLI described below.

This offline tool signs update artifacts without either desktop runtime. It
imports the base64-wrapped signing keys used by the former Tauri CLI and emits
the same wrapped Minisign signature format. A signed comment binds each artifact
to its release version. Signatures are checked against the configured public key
before replacing the `.sig` sidecar.

```sh
cargo test --locked -p newport-release --features signing
cargo run --locked --release -p newport-release --features signing -- sign /path/to/artifact 0.3.1 /path/to/public.key
```

The public-key file contains the base64-wrapped public key. Provide the private
key through `NEWPORT_SIGNING_PRIVATE_KEY` or `NEWPORT_SIGNING_PRIVATE_KEY_PATH`,
and its password through `NEWPORT_SIGNING_PRIVATE_KEY_PASSWORD` (empty by default).
Keys and passwords are never command-line arguments; this tool never prompts.

The packaging scripts use `scripts/sign-update.mjs`, which selects
`resources/updater.json` as the trust root and strips updater credentials before
building the tool. Existing `TAURI_SIGNING_PRIVATE_KEY` and password secret names
remain accepted by that wrapper. No Tauri executable is used for signing.

This authenticates updater packages, not OS code signatures. macOS Developer ID
signing/notarization, Windows distribution acceptance, profile upgrades and
rollback remain separate release requirements. No update is published here.

## Needle release identity

`newport-release needle <agent-directory> <version> <public-key-file>` signs a
manifest binding both Linux payload hashes to one release version. The output
is `needle-release.json`; its base64 envelope preserves the exact signed bytes.
`scripts/sign-needle-release.mjs` verifies the existing CI artifact before
signing with the same trust root and credential isolation as desktop updates.
The release workflow records the resulting artifact and supplies it to both
desktop builds.

Distribution builds set `NEWPORT_REQUIRE_NEEDLE_RELEASE=1`. The backend build
checks the signature, both binary hashes, and the desktop version before
embedding the metadata. Development builds may omit the file, but cannot use
stale or invalid metadata. Rebuilding Linux agents locally removes the previous
release identity. Unsigned builds have no asserted release ordering.

Explicit deployment records the signed envelope beside the immutable remote
binary. Discovery only reads it and verifies the signature and observed binary
hash. An older desktop refuses to replace a verified newer release, including
when resuming an older candidate. Explicit recovery of a recorded deployment is
a separate rollback operation. No remote version string is trusted by itself.

The Windows-only `native_package_runtime` Rust integration test exercises a real NSIS
candidate in an isolated install directory, including locked-file rollback,
interrupted-installer recovery, upgrade retry, and profile-preserving uninstall.
It is ignored by default because it changes current-user installation registration. See
[the test guide](../../docs/testing.md) for its explicit invocation and preflight.

## Dependency boundary

`newport_release::needle` owns the manifest types, signature verification and
hash-bound version comparison previously in `newport-needle-release`. The default
build has no signing feature. The backend uses this same verification-only API
at build time and runtime; it does not depend on the private-key signing library.
The CLI and its tests require `--features signing`, enabled explicitly by the
release wrapper and CI. No signature format, trust root or upgrade policy changed.
