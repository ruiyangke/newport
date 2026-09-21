#!/bin/bash
set -euo pipefail
# Restricted Keychain entitlements require a provisioned app bundle, including in dev.
binary="$1"
shift
if [[ "$(basename "$binary")" == porthop ]]; then
  : "${PORTHOP_SIGNING_IDENTITY:?Set your local code-signing identity}"
  : "${PORTHOP_PROVISIONING_PROFILE:?Set the Mac development provisioning profile path}"
  : "${PORTHOP_SIGNING_ENTITLEMENTS:?Set the matching Keychain entitlements path}"
  binary_dir="$(cd "$(dirname "$binary")" && pwd)"
  bundle="$binary_dir/Porthop.app"
  mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
  cp "$binary" "$bundle/Contents/MacOS/porthop"
  cp "$PORTHOP_PROVISIONING_PROFILE" "$bundle/Contents/embedded.provisionprofile"
  script_dir="$(cd "$(dirname "$0")" && pwd)"
  cp "$script_dir/../src-tauri/icons/icon.icns" "$bundle/Contents/Resources/icon.icns"
  cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>ke.ry.porthop</string>
<key>CFBundleExecutable</key><string>porthop</string>
<key>CFBundleName</key><string>Porthop</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIconFile</key><string>icon.icns</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>0.2.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>LSMinimumSystemVersion</key><string>14.0</string>
</dict></plist>
PLIST
  /usr/bin/codesign --force --sign "$PORTHOP_SIGNING_IDENTITY" \
    --entitlements "$PORTHOP_SIGNING_ENTITLEMENTS" --timestamp=none "$bundle"
  /usr/bin/codesign --verify --strict "$bundle"
  binary="$bundle/Contents/MacOS/porthop"
fi
exec "$binary" "$@"
