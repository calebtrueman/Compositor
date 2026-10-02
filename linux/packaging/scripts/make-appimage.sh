#!/usr/bin/env bash
# Builds an AppImage from a release binary:  make-appimage.sh <compositor-binary> <out.AppImage>
#
# The binary links only glibc and libgcc_s; GL, EGL, X11, Wayland and xkbcommon are loaded with
# dlopen from the host (as they must be, to match the host's GPU driver), so nothing is bundled.
# The result uses the static type2 runtime: it needs no libfuse2 on the host, and falls back to
# extracting itself when FUSE is unavailable (APPIMAGE_EXTRACT_AND_RUN=1 forces that).
set -euo pipefail

BIN="$1"
OUT="$2"
APPIMAGETOOL_VERSION="${APPIMAGETOOL_VERSION:-1.9.1}"
RUNTIME_VERSION="${RUNTIME_VERSION:-20251108}"
APP_ID=io.github.calebtrueman.Compositor
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ARCH="$(uname -m)"
TOOLS="${APPIMAGE_TOOLS_DIR:-$HOME/.cache/compositor-appimage}"
GH=https://github.com/AppImage

mkdir -p "$TOOLS"
TOOL="$TOOLS/appimagetool-$APPIMAGETOOL_VERSION-$ARCH.AppImage"
RUNTIME="$TOOLS/runtime-$RUNTIME_VERSION-$ARCH"
[ -s "$TOOL" ] || curl -fsSL -o "$TOOL" "$GH/appimagetool/releases/download/$APPIMAGETOOL_VERSION/appimagetool-$ARCH.AppImage"
[ -s "$RUNTIME" ] || curl -fsSL -o "$RUNTIME" "$GH/type2-runtime/releases/download/$RUNTIME_VERSION/runtime-$ARCH"
chmod +x "$TOOL"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
APPDIR="$WORK/Compositor.AppDir"
bash "$HERE/stage.sh" "$BIN" "$APPDIR" /usr
cp "$APPDIR/usr/share/applications/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
cp "$APPDIR/usr/share/icons/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/$APP_ID.png"
cp "$APPDIR/$APP_ID.png" "$APPDIR/.DirIcon"
cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/compositor" "$@"
EOF
chmod +x "$APPDIR/AppRun"

# --appimage-extract-and-run: build machines and containers usually have no FUSE.
ARCH="$ARCH" "$TOOL" --appimage-extract-and-run --no-appstream --runtime-file "$RUNTIME" "$APPDIR" "$OUT"
chmod +x "$OUT"
