#!/bin/sh
set -euo pipefail

# Generate .icns from SVG using macOS tools
# Requires: rsvg-convert (from librsvg) or cairosvg, or qlmanage

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
SVG="$ROOT_DIR/assets/icon.svg"
ICONSET="$ROOT_DIR/assets/AppIcon.iconset"
ICNS="$ROOT_DIR/macos/AppIcon.icns"

if ! command -v rsvg-convert >/dev/null 2>&1; then
    echo "rsvg-convert not found. Install with: brew install librsvg"
    exit 1
fi

mkdir -p "$ICONSET"

# Generate all required sizes
for size in 16 32 64 128 256 512 1024; do
    rsvg-convert -w "$size" -h "$size" "$SVG" -o "$ICONSET/icon_${size}x${size}.png"
done

# Create @2x variants (iconutil expects specific naming)
cp "$ICONSET/icon_32x32.png"   "$ICONSET/icon_16x16@2x.png"
cp "$ICONSET/icon_64x64.png"   "$ICONSET/icon_32x32@2x.png"
cp "$ICONSET/icon_256x256.png" "$ICONSET/icon_128x128@2x.png"
cp "$ICONSET/icon_512x512.png" "$ICONSET/icon_256x256@2x.png"
cp "$ICONSET/icon_1024x1024.png" "$ICONSET/icon_512x512@2x.png"

# Remove non-standard sizes
rm -f "$ICONSET/icon_64x64.png" "$ICONSET/icon_1024x1024.png"

iconutil -c icns "$ICONSET" -o "$ICNS"
rm -rf "$ICONSET"

echo "Generated $ICNS"
