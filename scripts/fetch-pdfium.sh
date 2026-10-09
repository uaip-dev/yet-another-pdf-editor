#!/usr/bin/env bash
# Downloads the prebuilt PDFium shared library (bblanchon/pdfium-binaries,
# BSD/Apache) into src-tauri/pdfium/. Usage: fetch-pdfium.sh <platform> [tag]
#   platform: win-x64 | mac-arm64 | mac-x64 | linux-x64 | linux-arm64
set -euo pipefail
PLATFORM="${1:?platform required, e.g. linux-x64}"
TAG="${2:-chromium/8086}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/src-tauri/pdfium"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
URL="https://github.com/bblanchon/pdfium-binaries/releases/download/$TAG/pdfium-$PLATFORM.tgz"
echo "Downloading $URL"
curl -fsSL "$URL" -o "$TMP/pdfium.tgz"
tar -xzf "$TMP/pdfium.tgz" -C "$TMP"
mkdir -p "$DEST"
case "$PLATFORM" in
  win-*) cp "$TMP/bin/pdfium.dll" "$DEST/" ;;
  mac-*) cp "$TMP/lib/libpdfium.dylib" "$DEST/" ;;
  *) cp "$TMP/lib/libpdfium.so" "$DEST/" ;;
esac
cp "$TMP/LICENSE" "$DEST/LICENSE.pdfium" 2>/dev/null || true
[ -f "$TMP/VERSION" ] && cp "$TMP/VERSION" "$DEST/VERSION" && cat "$TMP/VERSION"
echo "PDFium installed to $DEST"
