#!/bin/sh
set -eu

# Keep the generated set focused on the desktop targets configured by Tauri.
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

bunx tauri icon assets/lazed-icon.svg -o src-tauri/icons
rm -rf src-tauri/icons/android src-tauri/icons/ios
rm -f src-tauri/icons/StoreLogo.png src-tauri/icons/Square*Logo.png
