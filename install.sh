#!/bin/sh
# One-line installer for DataBug on macOS and Linux:
#   curl -fsSL https://raw.githubusercontent.com/hayes-roach/DataBug/main/install.sh | sh
# Downloads the latest release and installs it:
#   macOS → DataBug.app in /Applications (or ~/Applications without write access)
#   Linux → ~/.local/bin/databug plus an applications-menu entry
# Pass --uninstall to remove it. DATABUG_VERSION=v0.2.0 picks a release;
# DATABUG_DOWNLOAD_BASE points at a mirror; DATABUG_INSTALL_DIR overrides
# the macOS install folder; DATABUG_NO_OPEN=1 skips launching it.
set -eu

REPO="hayes-roach/DataBug"
VERSION="${DATABUG_VERSION:-latest}"
if [ "$VERSION" = latest ]; then
  BASE="https://github.com/$REPO/releases/latest/download"
else
  BASE="https://github.com/$REPO/releases/download/$VERSION"
fi
BASE="${DATABUG_DOWNLOAD_BASE:-$BASE}"

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }
fetch() {
  if command -v curl >/dev/null 2>&1; then curl -fL --progress-bar "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then wget -q --show-progress "$1" -O "$2"
  else fail "curl or wget is required"; fi
}

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

case "$(uname -s)" in
  Darwin)
    if [ -n "${DATABUG_INSTALL_DIR:-}" ]; then DEST="$DATABUG_INSTALL_DIR"; mkdir -p "$DEST"
    elif [ -w /Applications ]; then DEST=/Applications; else DEST="$HOME/Applications"; mkdir -p "$DEST"; fi
    if [ "${1:-}" = "--uninstall" ]; then
      rm -rf "/Applications/DataBug.app" "$HOME/Applications/DataBug.app"
      say "DataBug removed. Settings are kept in ~/Library/Application Support/dev.DataBug.DataBug."
      exit 0
    fi
    say "Downloading DataBug for macOS…"
    fetch "$BASE/DataBug-macos-universal.zip" "$TMP/databug.zip"
    ditto -x -k "$TMP/databug.zip" "$TMP/x"
    APP=$(find "$TMP/x" -maxdepth 3 -name DataBug.app -type d | head -1)
    [ -n "$APP" ] || fail "DataBug.app not found in the download"
    osascript -e 'quit app "DataBug"' >/dev/null 2>&1 || true
    rm -rf "$DEST/DataBug.app"
    ditto "$APP" "$DEST/DataBug.app"
    # downloaded with curl, so not quarantined; clear it anyway in case
    xattr -dr com.apple.quarantine "$DEST/DataBug.app" 2>/dev/null || true
    say "DataBug installed in $DEST."
    [ -n "${DATABUG_NO_OPEN:-}" ] || open "$DEST/DataBug.app" || true
    ;;
  Linux)
    if [ "${1:-}" = "--uninstall" ]; then
      rm -f "$HOME/.local/bin/databug" "$HOME/.local/share/applications/databug.desktop" "$HOME/.local/share/icons/hicolor/256x256/apps/databug.png"
      rm -rf "$HOME/.local/bin/logos"
      say "DataBug removed. Settings are kept in ~/.config/databug."
      exit 0
    fi
    case "$(uname -m)" in
      x86_64 | amd64) ;;
      *) fail "prebuilt DataBug downloads are x86_64 only; build from source with: cargo build --release" ;;
    esac
    say "Downloading DataBug for Linux…"
    fetch "$BASE/DataBug-linux-x86_64.tar.gz" "$TMP/databug.tar.gz"
    tar xzf "$TMP/databug.tar.gz" -C "$TMP"
    sh "$TMP/DataBug/install.sh"
    ;;
  *)
    fail "this script is for macOS and Linux. On Windows run in PowerShell: irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex"
    ;;
esac
