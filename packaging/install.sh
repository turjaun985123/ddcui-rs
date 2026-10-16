#!/usr/bin/env bash
# Install ddcui-rs from an unpacked release tarball.
#
#   ./install.sh                 # install into /usr/local
#   PREFIX=/usr ./install.sh     # install into /usr (a .deb does this)
#   DESTDIR=/tmp/pkg ./install.sh
set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
DESTDIR="${DESTDIR:-}"
APP_ID="io.github.jerinshop7_maker.DdcuiRs"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

install_file() {
  local src="$1" dst="$2" mode="$3"
  install -D -m "$mode" "$src" "$DESTDIR$PREFIX/$dst"
}

install_file "$here/ddcui-rs"                    "bin/ddcui-rs"                      0755
install_file "$here/$APP_ID.desktop"             "share/applications/$APP_ID.desktop" 0644
install_file "$here/$APP_ID.metainfo.xml"        "share/metainfo/$APP_ID.metainfo.xml" 0644

for size in 16 32 48 64 128 256; do
  install_file "$here/icons/video-display-${size}px.png" \
    "share/icons/hicolor/${size}x${size}/apps/$APP_ID.png" 0644
done

# Refresh the desktop/icon caches when installing on a live system.
if [ -z "$DESTDIR" ] && command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$PREFIX/share/applications" || true
fi
if [ -z "$DESTDIR" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" || true
fi

echo "Installed ddcui-rs into $DESTDIR$PREFIX"
echo "Run 'ddcui-rs' (ensure the 'ddcutil' program is installed and /dev/i2c-* is accessible)."
