#!/bin/bash
set -euo pipefail
# Restricted Keychain entitlements require a provisioned app bundle, including in dev.
binary="$1"
shift
if [[ "$(basename "$binary")" == newport ]]; then
  : "${NEWPORT_SIGNING_IDENTITY:?Set your local code-signing identity}"
  : "${NEWPORT_PROVISIONING_PROFILE:?Set the Mac development provisioning profile path}"
  : "${NEWPORT_SIGNING_ENTITLEMENTS:?Set the matching Keychain entitlements path}"
  binary_dir="$(cd "$(dirname "$binary")" && pwd)"
  script_dir="$(cd "$(dirname "$0")" && pwd)"
  node "$script_dir/check-signing.mjs" "$NEWPORT_SIGNING_ENTITLEMENTS" "$NEWPORT_PROVISIONING_PROFILE"
  bundle_info="$(node "$script_dir/native-build-info.mjs" "$binary")"
  bundle="$binary_dir/Newport.app"
  mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
  mkdir -p "$bundle/Contents/Resources/Fonts"
  cp "$binary" "$bundle/Contents/MacOS/newport"
  mkdir -p "$bundle/Contents/Resources/agents"
  cp "$binary_dir/agents/newport-agent-x86_64.gz" "$bundle/Contents/Resources/agents/"
  cp "$binary_dir/agents/newport-agent-aarch64.gz" "$bundle/Contents/Resources/agents/"
  rm -rf "$bundle/Contents/Resources/grammars"
  cp -R "$binary_dir/grammars" "$bundle/Contents/Resources/grammars"
  for library in "$bundle/Contents/Resources/grammars/"*.dylib; do
    /usr/bin/codesign --force --sign "$NEWPORT_SIGNING_IDENTITY" --timestamp=none "$library"
  done
  cp "$NEWPORT_PROVISIONING_PROFILE" "$bundle/Contents/embedded.provisionprofile"
  cp "$script_dir/../resources/icons/icon.icns" "$bundle/Contents/Resources/icon.icns"
  cp "$script_dir/../resources/fonts/SymbolsNerdFontMono-Regular.ttf" "$bundle/Contents/Resources/Fonts/"
  cp "$script_dir/../resources/fonts/NERD-FONTS-LICENSE" "$bundle/Contents/Resources/"
  printf '%s\n' "$bundle_info" > "$bundle/Contents/Info.plist"
  /usr/bin/codesign --force --sign "$NEWPORT_SIGNING_IDENTITY" \
    --entitlements "$NEWPORT_SIGNING_ENTITLEMENTS" --timestamp=none "$bundle"
  /usr/bin/codesign --verify --strict "$bundle"
  binary="$bundle/Contents/MacOS/newport"
fi
exec "$binary" "$@"
