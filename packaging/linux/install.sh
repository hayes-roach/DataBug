#!/bin/sh
# Installs DataBug for the current user: binary, icon and menu entry.
set -e
dir="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" "$HOME/.local/share/icons/hicolor/256x256/apps"
install -m 755 "$dir/databug" "$HOME/.local/bin/databug"
if [ -d "$dir/logos" ]; then
  mkdir -p "$HOME/.local/bin/logos"
  cp -n "$dir"/logos/* "$HOME/.local/bin/logos/" 2>/dev/null || true
fi
install -m 644 "$dir/databug.png" "$HOME/.local/share/icons/hicolor/256x256/apps/databug.png"
install -m 644 "$dir/databug.desktop" "$HOME/.local/share/applications/databug.desktop"
echo "DataBug installed. Make sure ~/.local/bin is on your PATH."
