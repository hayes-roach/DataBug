#!/bin/sh
# One-line installer and updater for DataBug on macOS and Linux:
#   curl -fsSL https://raw.githubusercontent.com/hayes-roach/DataBug/main/install.sh | sh
# Installs the latest release, or updates an existing install in place
# (settings, connections and history are kept):
#   macOS → DataBug.app in /Applications (or ~/Applications without write access)
#   Linux → ~/.local/bin/databug plus an applications-menu entry
# Already on the latest version? It says so and stops; pass --force to
# reinstall anyway. Pass --uninstall to remove DataBug.
#
# A first install asks which folder to use (press Enter for the default);
# updates and --uninstall find it again. DATABUG_INSTALL_DIR=/some/folder
# picks the folder without asking; --yes (or DATABUG_YES=1) takes the
# default without asking.
#
# Options for testing and mirrors: DATABUG_VERSION=v0.2.0 picks a release;
# DATABUG_DOWNLOAD_BASE points at a mirror; DATABUG_UPDATE_URL is the
# release-info URL; DATABUG_NO_OPEN=1 skips launching it; on macOS,
# DATABUG_APP_DIRS lists the folders searched for copies, separated by
# colons (default /Applications:$HOME/Applications).
#
# After installing or updating, other copies of DataBug that this script
# can find are removed, so only the new one is left.
set -eu

REPO="hayes-roach/DataBug"
VERSION="${DATABUG_VERSION:-latest}"
if [ "$VERSION" = latest ]; then
  BASE="https://github.com/$REPO/releases/latest/download"
  INFO="https://api.github.com/repos/$REPO/releases/latest"
else
  BASE="https://github.com/$REPO/releases/download/$VERSION"
  INFO=""
fi
BASE="${DATABUG_DOWNLOAD_BASE:-$BASE}"
INFO="${DATABUG_UPDATE_URL:-$INFO}"

FORCE=0
ACTION=install
for arg in "$@"; do
  case "$arg" in
    --force) FORCE=1 ;;
    --uninstall) ACTION=uninstall ;;
    --yes | -y) DATABUG_YES=1 ;;
    *) printf 'Unknown option: %s\n' "$arg" >&2; exit 2 ;;
  esac
done

say() { printf '\033[1m%s\033[0m\n' "$*"; }
note() { printf '%s\n' "$*"; }
fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }
fetch() {
  if command -v curl >/dev/null 2>&1; then curl -fL --progress-bar "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then wget -q --show-progress "$1" -O "$2"
  else fail "curl or wget is required"; fi
}
fetch_quiet() {
  if command -v curl >/dev/null 2>&1; then curl -fsSL "$1"
  elif command -v wget >/dev/null 2>&1; then wget -qO- "$1"
  else return 1; fi
}

# Ask which folder to install into, showing the default; Enter keeps it.
# Asks only on a terminal (it reads /dev/tty, so `curl … | sh` works);
# otherwise, or with DATABUG_YES, it takes the default. DATABUG_INSTALL_DIR
# answers without asking.
choose_dir() {
  default="$1"
  what="$2"
  if [ -n "${DATABUG_INSTALL_DIR:-}" ]; then
    answer="$DATABUG_INSTALL_DIR"
  elif [ -z "${DATABUG_YES:-}" ] && (: </dev/tty) 2>/dev/null && [ -w /dev/tty ]; then
    printf '%s\n  [%s]: ' "$what" "$default" >/dev/tty
    IFS= read -r answer </dev/tty || answer=""
  else
    answer=""
  fi
  [ -n "$answer" ] || answer="$default"
  # a typed ~ arrives as text, so expand it here
  # shellcheck disable=SC2088
  case "$answer" in
    "~") answer="$HOME" ;;
    "~/"*) answer="$HOME/${answer#\~/}" ;;
  esac
  case "$answer" in
    /*) ;;
    *) answer="$(pwd)/$answer" ;;
  esac
  mkdir -p "$answer" 2>/dev/null || fail "Couldn't create $answer"
  [ -w "$answer" ] || fail "You can't write to $answer. Pick a folder in your home folder, or run with sudo."
  printf '%s' "$answer"
}

# Remove copies of DataBug.app other than $1: in the usual folders, the
# folder an earlier run remembered ($2), and backups left by an interrupted
# update. Only bundles that really are DataBug (by bundle id) are touched.
app_dirs() {
  if [ -n "${DATABUG_APP_DIRS:-}" ]; then printf '%s\n' "$DATABUG_APP_DIRS" | tr ':' '\n'
  else printf '%s\n%s\n' /Applications "$HOME/Applications"; fi
}
remove_other_apps() {
  keep="$1"
  { app_dirs; printf '%s\n' "$2"; } | while IFS= read -r d; do
    [ -n "$d" ] || continue
    for app in "$d/DataBug.app" "$d/DataBug.app.previous"; do
      [ -d "$app" ] || continue
      [ "$app" = "$keep" ] && continue
      id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist" 2>/dev/null || true)
      [ "$id" = "dev.databug.DataBug" ] || continue
      if rm -rf "$app" 2>/dev/null; then note "Removed an older copy: $app"; else note "Couldn't remove an older copy at $app (remove it by hand)."; fi
    done
  done
}

# Where an earlier run of this script installed DataBug (empty if unknown).
remembered() { [ -f "$1" ] && head -n 1 "$1" || true; }
remember() { mkdir -p "$(dirname "$1")" && printf '%s\n' "$2" > "$1"; }

# Version of the release being installed, e.g. 0.2.0 (empty if unknown).
target_version() {
  if [ "$VERSION" != latest ]; then
    printf '%s' "$VERSION" | sed 's/^[vV]//'
    return
  fi
  [ -n "$INFO" ] || return 0
  fetch_quiet "$INFO" 2>/dev/null | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"[vV]\{0,1\}\([^"]*\)".*/\1/p' | head -1
}

# Decide between install / update / up to date. Sets WANT and prints what happens.
plan() {
  current="$1"
  WANT=$(target_version || true)
  if [ -n "$current" ] && [ -n "$WANT" ] && [ "$current" = "$WANT" ] && [ "$FORCE" = 0 ]; then
    say "DataBug $current is already installed and up to date."
    note "Run again with --force to reinstall it:  curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh -s -- --force"
    exit 0
  fi
  if [ -n "$current" ]; then
    say "Updating DataBug ${current} → ${WANT:-latest}…"
  else
    say "Installing DataBug ${WANT:-latest}…"
  fi
}

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

case "$(uname -s)" in
  Darwin)
    MEMO="$HOME/Library/Application Support/dev.DataBug.DataBug/install-dir"
    SAVED=$(remembered "$MEMO")
    # an existing copy is updated where it is; a first install asks where to go
    if [ -n "${DATABUG_INSTALL_DIR:-}" ]; then DEST=$(choose_dir "$DATABUG_INSTALL_DIR" "") || exit 1
    elif [ -n "$SAVED" ] && [ -d "$SAVED/DataBug.app" ]; then DEST="$SAVED"
    else
      DEST=$(app_dirs | while IFS= read -r d; do if [ -d "$d/DataBug.app" ]; then printf '%s' "$d"; break; fi; done)
    fi
    if [ -n "$DEST" ]; then :
    elif [ "$ACTION" = uninstall ]; then DEST=""
    else
      if [ -w /Applications ]; then DEFAULT=/Applications; else DEFAULT="$HOME/Applications"; fi
      DEST=$(choose_dir "$DEFAULT" "Install DataBug in which folder?") || exit 1
    fi
    TARGET="$DEST/DataBug.app"
    if [ "$ACTION" = uninstall ]; then
      [ -n "$DEST" ] || fail "DataBug isn't installed (no DataBug.app in /Applications, ~/Applications or a folder this script installed to)."
      rm -rf "$TARGET"
      rm -f "$MEMO"
      say "DataBug removed from $DEST. Settings are kept in ~/Library/Application Support/dev.DataBug.DataBug."
      exit 0
    fi
    CURRENT=""
    if [ -f "$TARGET/Contents/Info.plist" ]; then
      CURRENT=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$TARGET/Contents/Info.plist" 2>/dev/null || true)
    fi
    plan "$CURRENT"
    fetch "$BASE/DataBug-macos-universal.zip" "$TMP/databug.zip"
    ditto -x -k "$TMP/databug.zip" "$TMP/x"
    APP=$(find "$TMP/x" -maxdepth 3 -name DataBug.app -type d | head -1)
    [ -n "$APP" ] || fail "DataBug.app not found in the download"

    # Quit a running copy and wait until it has really exited, so macOS
    # doesn't try to hand the relaunch to a process that's shutting down.
    WAS_RUNNING=0
    if pgrep -f "$TARGET/Contents/MacOS/databug" >/dev/null 2>&1; then
      WAS_RUNNING=1
      note "Closing DataBug…"
      osascript -e 'tell application id "dev.databug.DataBug" to quit' >/dev/null 2>&1 || true
      i=0
      while pgrep -f "$TARGET/Contents/MacOS/databug" >/dev/null 2>&1; do
        i=$((i + 1))
        if [ "$i" -gt 50 ]; then
          fail "DataBug is still open (maybe waiting on an unsaved dialog). Quit it, then run the installer again."
        fi
        sleep 0.2
      done
    fi

    # Replace the app, keeping the old copy until the new one is in place.
    if [ -d "$TARGET" ]; then
      rm -rf "$TARGET.previous"
      mv "$TARGET" "$TARGET.previous"
    fi
    if ditto "$APP" "$TARGET"; then
      rm -rf "$TARGET.previous"
    else
      rm -rf "$TARGET"
      [ -d "$TARGET.previous" ] && mv "$TARGET.previous" "$TARGET"
      fail "Couldn't copy DataBug into $DEST"
    fi
    # downloaded with curl, so not quarantined; clear it anyway in case
    xattr -dr com.apple.quarantine "$TARGET" 2>/dev/null || true
    remove_other_apps "$TARGET" "$SAVED"
    remember "$MEMO" "$DEST"
    # tell macOS about the new copy before opening it
    LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
    [ -x "$LSREGISTER" ] && "$LSREGISTER" -f "$TARGET" >/dev/null 2>&1 || true

    NEW=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$TARGET/Contents/Info.plist" 2>/dev/null || echo "")
    if [ -n "$CURRENT" ]; then
      say "DataBug updated to ${NEW:-the latest version} in $DEST. Your connections and settings are kept."
    else
      say "DataBug ${NEW} installed in $DEST."
    fi
    if [ -z "${DATABUG_NO_OPEN:-}" ]; then
      opened=0
      for attempt in 1 2 3; do
        if open "$TARGET" >/dev/null 2>&1; then opened=1; break; fi
        sleep 1
      done
      [ "$opened" = 1 ] || note "Open DataBug from your Applications folder to get started."
    fi
    [ "$WAS_RUNNING" = 1 ] && [ -n "${DATABUG_NO_OPEN:-}" ] && note "DataBug was closed for the update; open it again from Applications."
    ;;
  Linux)
    # the package's own install.sh remembers the folder in this file
    MEMO="${XDG_CONFIG_HOME:-$HOME/.config}/databug/install-dir"
    BIN=$(remembered "$MEMO")
    [ -n "$BIN" ] && [ -x "$BIN/databug" ] || BIN="$HOME/.local/bin"
    [ -n "${DATABUG_INSTALL_DIR:-}" ] && { BIN=$(choose_dir "$DATABUG_INSTALL_DIR" "") || exit 1; }
    if [ "$ACTION" = uninstall ]; then
      rm -f "$BIN/databug" "$HOME/.local/share/applications/databug.desktop" "$HOME/.local/share/icons/hicolor/256x256/apps/databug.png" "$MEMO"
      rm -rf "$BIN/logos"
      say "DataBug removed from $BIN. Settings are kept in ~/.config/databug."
      exit 0
    fi
    case "$(uname -m)" in
      x86_64 | amd64) ;;
      *) fail "prebuilt DataBug downloads are x86_64 only; build from source with: cargo build --release" ;;
    esac
    CURRENT=""
    if [ -x "$BIN/databug" ]; then
      CURRENT=$("$BIN/databug" --version 2>/dev/null | sed -n 's/^databug //p' || true)
    else
      BIN=$(choose_dir "$HOME/.local/bin" "Install the databug program in which folder?") || exit 1
    fi
    plan "$CURRENT"
    fetch "$BASE/DataBug-linux-x86_64.tar.gz" "$TMP/databug.tar.gz"
    tar xzf "$TMP/databug.tar.gz" -C "$TMP"
    DATABUG_BIN_DIR="$BIN" sh "$TMP/DataBug/install.sh"
    [ -n "$CURRENT" ] && say "Updated from $CURRENT. Restart DataBug if it's open to use the new version."
    ;;
  *)
    fail "this script is for macOS and Linux. On Windows run in PowerShell: irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex"
    ;;
esac
exit 0
