# Application resources

`app.json` defines the product name, version, and production identifier. Native
packaging validates executable build receipts against this metadata.

`icons/` and `fonts/` supply native UI assets. Preserve their license files when
packaging. `agents/` holds the Linux Needle builds and release metadata; packaging
compresses the binaries into resources, and the backend verifies them before
installation. Do not replace signed resources during unrelated development.

`updater.json` holds the release endpoint and public verification key used by
existing installations. Keep that key compatible across releases.
`test-fixtures/updater/` contains inert payloads and public signatures for tests,
not executable updates or production private keys.
