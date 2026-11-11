#!/usr/bin/env bash
# Build Atype (release) and zip it for download. Does not touch /Applications.
#   cd apps/mac && bun run app:package
# Output: apps/mac/Atype-<version>-macos-arm64.zip (Apple silicon, ad-hoc signed, not notarized)
set -euo pipefail
cd "$(dirname "$0")/.."

bun install
bun run tauri build

APP="src-tauri/target/release/bundle/macos/Atype.app"
[ -d "$APP" ] || { echo "Build output not found: $APP" >&2; exit 1; }

VERSION="$(sed -n 's/^[[:space:]]*"version":[[:space:]]*"\([^"]*\)".*/\1/p' src-tauri/tauri.conf.json | head -n 1)"
[ -n "$VERSION" ] || { echo "Cannot read version from src-tauri/tauri.conf.json" >&2; exit 1; }

ZIP="Atype-${VERSION}-macos-arm64.zip"
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"

echo
echo "Packaged: $(pwd)/$ZIP"
