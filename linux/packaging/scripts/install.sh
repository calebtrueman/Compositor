#!/bin/sh
# Installs Compositor from this folder.
#
#   ./install.sh                 per-user, into ~/.local (no root needed)
#   sudo ./install.sh            system-wide, into /usr/local
#   ./install.sh --prefix DIR    anywhere else
#   ./install.sh --uninstall     removes it again (pass the same --prefix / sudo as installing)
#
# The command is `compositor`. Runtime libraries (OpenGL/EGL, X11 or Wayland, xkbcommon) come
# from the system; every desktop install already has them.
set -eu

APP_ID=io.github.calebtrueman.Compositor
HERE=$(cd "$(dirname "$0")" && pwd)

if [ "$(id -u)" -eq 0 ]; then PREFIX=/usr/local; else PREFIX="$HOME/.local"; fi
UNINSTALL=0
while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --prefix=*) PREFIX="${1#--prefix=}"; shift ;;
    --uninstall) UNINSTALL=1; shift ;;
    -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
done

SIZES="16 32 64 128 256 512"
FILES="bin/compositor share/applications/$APP_ID.desktop share/metainfo/$APP_ID.metainfo.xml share/mime/packages/$APP_ID.xml"
for s in $SIZES; do FILES="$FILES share/icons/hicolor/${s}x${s}/apps/$APP_ID.png"; done

refresh() {
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true
  command -v update-mime-database >/dev/null 2>&1 && update-mime-database "$PREFIX/share/mime" 2>/dev/null || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -f "$PREFIX/share/icons/hicolor/index.theme" ] \
    && gtk-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
}

if [ "$UNINSTALL" -eq 1 ]; then
  for f in $FILES; do rm -f "$PREFIX/$f"; done
  refresh
  echo "Removed Compositor from $PREFIX"
  exit 0
fi

for f in $FILES; do
  case "$f" in bin/*) mode=755 ;; *) mode=644 ;; esac
  mkdir -p "$PREFIX/$(dirname "$f")"
  cp "$HERE/$f" "$PREFIX/$f"
  chmod "$mode" "$PREFIX/$f"
done
# Point the menu entry at the installed binary, so it launches even when $PREFIX/bin is not on
# PATH (as ~/.local/bin often is not for graphical sessions).
sed -i "s|^Exec=compositor|Exec=$PREFIX/bin/compositor|" "$PREFIX/share/applications/$APP_ID.desktop"
refresh

echo "Installed Compositor into $PREFIX"
case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) echo "Note: $PREFIX/bin is not on your PATH; the app menu entry still works." ;;
esac
