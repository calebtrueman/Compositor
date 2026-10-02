#!/usr/bin/env bash
# Lays out Compositor's installed files under a prefix, the same layout the .deb and .rpm use:
#   stage.sh <compositor-binary> <destdir> [prefix=/usr]
# Used for the tarball, AppImage and Flatpak, and by the Arch PKGBUILDs' package() step.
set -euo pipefail

BIN="$1"
DESTDIR="$2"
PREFIX="${3-/usr}"        # may be "" (the tarball puts bin/ and share/ at its top level)
APP_ID=io.github.calebtrueman.Compositor
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PKG="$HERE/.."            # linux/packaging
ASSETS="$HERE/../../assets"
ROOT="$DESTDIR$PREFIX"

install -Dm755 "$BIN" "$ROOT/bin/compositor"
install -Dm644 "$PKG/$APP_ID.desktop" "$ROOT/share/applications/$APP_ID.desktop"
install -Dm644 "$PKG/$APP_ID.metainfo.xml" "$ROOT/share/metainfo/$APP_ID.metainfo.xml"
install -Dm644 "$PKG/compositor-project.xml" "$ROOT/share/mime/packages/$APP_ID.xml"
for size in 16 32 64 128 256 512; do
  install -Dm644 "$ASSETS/app-icon-$size.png" \
    "$ROOT/share/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
done
