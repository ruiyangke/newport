#!/bin/bash
# Build, sign, notarize, then package. Never publish an unnotarized updater.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${APPLE_SIGNING_IDENTITY:?Set the Developer ID Application identity}"
: "${NEWPORT_PROVISIONING_PROFILE:?Set the Developer ID profile path}"
: "${NEWPORT_SIGNING_ENTITLEMENTS:?Set the distribution entitlements path}"
: "${TAURI_SIGNING_PRIVATE_KEY:?Set the updater signing key path or contents}"
: "${APPLE_ID:?Set the notarization Apple ID}"
: "${APPLE_PASSWORD:?Set the notarization app-specific password}"
: "${APPLE_TEAM_ID:?Set the Apple team ID}"
python3 scripts/check-signing.py "$NEWPORT_SIGNING_ENTITLEMENTS" "$NEWPORT_PROVISIONING_PROFILE"
npm run tauri build -- --bundles app --target aarch64-apple-darwin
app="src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Newport.app"
cp "$NEWPORT_PROVISIONING_PROFILE" "$app/Contents/embedded.provisionprofile"
codesign --force --sign "$APPLE_SIGNING_IDENTITY" --options runtime --timestamp \
  --entitlements "$NEWPORT_SIGNING_ENTITLEMENTS" "$app"
codesign --verify --deep --strict "$app"
submission="src-tauri/target/release/notarization.zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$submission"
xcrun notarytool submit "$submission" --apple-id "$APPLE_ID" \
  --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" --wait
xcrun stapler staple "$app"
python3 scripts/package-update.py "$app" --output src-tauri/target/release/update
