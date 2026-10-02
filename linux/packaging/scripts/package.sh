#!/usr/bin/env bash
# Packages an already-built release binary (linux/target/release/compositor) for the machine's
# architecture into OUTDIR (default linux/dist):
#   compositor-image-editor_<ver>-1_<amd64|arm64>.deb
#   compositor-image-editor-<ver>-1.<x86_64|aarch64>.rpm
#   compositor-image-editor-<ver>-linux-<x86_64|aarch64>.tar.gz
#   Compositor-<ver>-<x86_64|aarch64>.AppImage
# Needs cargo-deb and cargo-generate-rpm (see setup-build-env.sh); the AppImage step downloads
# appimagetool. FORMATS="deb rpm tar appimage" (the default) picks which to make.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LINUX="$(cd "$HERE/../.." && pwd)"
REPO="$(cd "$LINUX/.." && pwd)"
OUT="${1:-$LINUX/dist}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"

VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$LINUX/Cargo.toml" | head -n1)"
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64) DEB_ARCH=amd64 ;;
  aarch64) DEB_ARCH=arm64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 1 ;;
esac
BIN="$LINUX/target/release/compositor"
[ -x "$BIN" ] || { echo "build first: cargo build --release --locked (in linux/)" >&2; exit 1; }
"$BIN" --version

PKG=compositor-image-editor
FORMATS=" ${FORMATS:-deb rpm tar appimage} "
cd "$LINUX"

if [[ $FORMATS == *" deb "* ]]; then
  echo "--- .deb"
  cargo deb --no-build --output "$OUT/${PKG}_${VERSION}-1_${DEB_ARCH}.deb"
fi

if [[ $FORMATS == *" rpm "* ]]; then
  echo "--- .rpm"
  cargo generate-rpm --output "$OUT/${PKG}-${VERSION}-1.${ARCH}.rpm"
fi

if [[ $FORMATS == *" tar "* ]]; then
  echo "--- .tar.gz"
  WORK="$(mktemp -d)"
  trap 'rm -rf "$WORK"' EXIT
  TARDIR="$WORK/${PKG}-${VERSION}-linux-${ARCH}"
  bash "$HERE/stage.sh" "$BIN" "$TARDIR" ""
  install -m755 "$HERE/install.sh" "$TARDIR/install.sh"
  install -m644 "$REPO/LICENSE" "$TARDIR/LICENSE"
  cat > "$TARDIR/README.txt" <<EOF
Compositor ${VERSION} for Linux (${ARCH})

Run it in place:   ./bin/compositor
Install per-user:  ./install.sh               (into ~/.local)
Install for all:   sudo ./install.sh          (into /usr/local)
Uninstall:         ./install.sh --uninstall   (same sudo/--prefix as installing)

Needs OpenGL (Mesa or a vendor driver) and an X11 or Wayland session.
https://github.com/calebtrueman/Compositor
EOF
  tar -C "$WORK" --owner=0 --group=0 -czf "$OUT/${PKG}-${VERSION}-linux-${ARCH}.tar.gz" "$(basename "$TARDIR")"
fi

if [[ $FORMATS == *" appimage "* ]]; then
  echo "--- AppImage"
  bash "$HERE/make-appimage.sh" "$BIN" "$OUT/Compositor-${VERSION}-${ARCH}.AppImage"
fi

ls -l "$OUT"
