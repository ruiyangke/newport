# Shared application resources

`app.json` holds the product name, version, and production identifier independently
of the UI. The native development bundle keeps its separate
`app.newport.native-test` identity. Native packaging validates executable build
receipts against this metadata.

The native app and retained React reference use `icons/` and `fonts/`. Retain the
font license when packaging. These assets are independent of the removed Tauri host.

`updater.json` preserves the release endpoint and public verification key used by
the released desktop app. `test-fixtures/updater/` contains inert payloads
and public signatures shared by native and packaging tests; none
is an executable update or a production private key.

`test-fixtures/legacy-desktop-release.json` pins the v0.3.0 desktop acceptance
baseline by release URL, asset name and SHA-256 digest. See the
[acceptance instructions](../docs/testing.md#released-desktop-baseline) before
running it. Pinning a baseline is not evidence that an upgrade has passed.
