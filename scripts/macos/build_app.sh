#!/bin/sh
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
APP_NAME="CCKit"
APP_DIR="$ROOT_DIR/dist/${APP_NAME}.app"
BIN_NAME="cckit"

cargo build --release --bin "$BIN_NAME"

mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
cp "$ROOT_DIR/macos/Info.plist" "$APP_DIR/Contents/Info.plist"
cp "$ROOT_DIR/macos/AppIcon.icns" "$APP_DIR/Contents/Resources/AppIcon.icns"
cp "$ROOT_DIR/target/release/$BIN_NAME" "$APP_DIR/Contents/MacOS/$BIN_NAME"
chmod +x "$APP_DIR/Contents/MacOS/$BIN_NAME"

echo "Built $APP_DIR"
