#!/bin/sh
# Installs DataBug for the current user: the app, its icon and a menu entry.
#   ./install.sh             install (or update)
#   ./install.sh --uninstall remove it again (settings and history are kept)
# The program goes in ~/.local/bin unless DATABUG_BIN_DIR names another
# folder; the folder is remembered, so updates go to the same place.
set -e
dir="$(cd "$(dirname "$0")" && pwd)"
memo="${XDG_CONFIG_HOME:-$HOME/.config}/databug/install-dir"
saved=""
[ -f "$memo" ] && saved=$(head -n 1 "$memo")
bin="${DATABUG_BIN_DIR:-${saved:-$HOME/.local/bin}}"
apps="$HOME/.local/share/applications"
icons="$HOME/.local/share/icons/hicolor/256x256/apps"

if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$bin/databug" "$apps/databug.desktop" "$icons/databug.png" "$memo"
  rm -rf "$bin/logos"
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" >/dev/null 2>&1 || true
  echo "DataBug removed. Your settings are still in ~/.config/databug."
  exit 0
fi

mkdir -p "$bin" "$apps" "$icons"
install -m 755 "$dir/databug" "$bin/databug"
if [ -d "$dir/logos" ]; then
  mkdir -p "$bin/logos"
  cp "$dir"/logos/* "$bin/logos/" 2>/dev/null || true
fi
install -m 644 "$dir/databug.png" "$icons/databug.png"
# absolute path, so the menu entry works even when ~/.local/bin isn't on PATH
sed "s|^Exec=.*|Exec=\"$bin/databug\"|" "$dir/databug.desktop" > "$apps/databug.desktop"
chmod 644 "$apps/databug.desktop"
mkdir -p "$(dirname "$memo")" && printf '%s\n' "$bin" > "$memo"
# only one copy: remove DataBug from the folder it was in before, if it moved
for old in "$saved" "$HOME/.local/bin"; do
  [ -n "$old" ] && [ "$old" != "$bin" ] && [ -x "$old/databug" ] || continue
  if "$old/databug" --version 2>/dev/null | grep -q '^databug '; then
    rm -f "$old/databug" && rm -rf "$old/logos" && echo "Removed the older copy in $old."
  fi
done
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true

echo "DataBug installed. Open it from your applications menu, or run: databug"
echo "DataBug is free and open source (Apache-2.0). Use at your own risk: it comes with no warranty and the authors aren't responsible for anything that happens from using it."
case ":$PATH:" in
  *":$bin:"*) ;;
  *) echo "Note: $bin isn't on your PATH, so the 'databug' command needs the full path." ;;
esac
