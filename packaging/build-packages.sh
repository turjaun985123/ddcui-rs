#!/usr/bin/env bash
# Build the release artifacts for ddcui-rs:
#   dist/ddcui-rs-<version>-x86_64-linux.tar.gz
#   dist/ddcui-rs_<version>_amd64.deb
#
# Usage: packaging/build-packages.sh <version> [target-dir]
# Expects an already-built release binary at <target-dir>/ddcui-rs.
set -euo pipefail

APP_ID="io.github.jerinshop7_maker.DdcuiRs"
VERSION="${1:?usage: build-packages.sh <version> [target-dir]}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${2:-$ROOT/target/release}"
DIST="$ROOT/dist"
BIN="$TARGET/ddcui-rs"

if [ ! -x "$BIN" ]; then
  echo "error: $BIN not found; run 'cargo build --release -p ddcui-gpui' first" >&2
  exit 1
fi

rm -rf "$DIST"
mkdir -p "$DIST"

# ---------- tarball ----------
name="ddcui-rs-${VERSION}-x86_64-linux"
stage="$DIST/$name"
mkdir -p "$stage/icons"

install -m 0755 "$BIN" "$stage/ddcui-rs"
install -m 0644 "$ROOT/packaging/$APP_ID.desktop" "$stage/"
install -m 0644 "$ROOT/packaging/$APP_ID.metainfo.xml" "$stage/"
install -m 0755 "$ROOT/packaging/install.sh" "$stage/install.sh"
install -m 0644 "$ROOT/LICENSE" "$stage/LICENSE"
install -m 0644 "$ROOT/README.md" "$stage/README.md"
for size in 16 32 48 64 128 256; do
  install -m 0644 "$ROOT/packaging/icons/video-display-${size}px.png" "$stage/icons/"
done
install -m 0644 "$ROOT/packaging/icons/COPYING" "$stage/icons/COPYING"

tar -C "$DIST" -czf "$DIST/$name.tar.gz" "$name"

# ---------- .deb ----------
deb="$DIST/deb"
mkdir -p "$deb/DEBIAN" \
         "$deb/usr/bin" \
         "$deb/usr/share/applications" \
         "$deb/usr/share/metainfo" \
         "$deb/usr/share/doc/ddcui-rs"

install -m 0755 "$BIN" "$deb/usr/bin/ddcui-rs"
install -m 0644 "$ROOT/packaging/$APP_ID.desktop" "$deb/usr/share/applications/"
install -m 0644 "$ROOT/packaging/$APP_ID.metainfo.xml" "$deb/usr/share/metainfo/"
for size in 16 32 48 64 128 256; do
  dir="$deb/usr/share/icons/hicolor/${size}x${size}/apps"
  mkdir -p "$dir"
  install -m 0644 "$ROOT/packaging/icons/video-display-${size}px.png" "$dir/$APP_ID.png"
done
install -m 0644 "$ROOT/README.md" "$deb/usr/share/doc/ddcui-rs/README.md"
gzip -9 -n -c "$ROOT/LICENSE" > "$deb/usr/share/doc/ddcui-rs/copyright.gz"
sed "s/@VERSION@/${VERSION}/g" "$ROOT/packaging/debian-control.in" > "$deb/DEBIAN/control"

dpkg-deb --build --root-owner-group "$deb" "$DIST/ddcui-rs_${VERSION}_amd64.deb" >/dev/null
rm -rf "$deb"

# ---------- AppImage (single portable file, no install) ----------
appdir="$DIST/AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" \
         "$appdir/usr/share/applications" \
         "$appdir/usr/share/metainfo" \
         "$appdir/usr/share/icons/hicolor/256x256/apps"

install -m 0755 "$BIN" "$appdir/usr/bin/ddcui-rs"
install -m 0644 "$ROOT/packaging/$APP_ID.desktop" "$appdir/usr/share/applications/"
install -m 0644 "$ROOT/packaging/$APP_ID.metainfo.xml" "$appdir/usr/share/metainfo/"
install -m 0644 "$ROOT/packaging/icons/video-display-256px.png" \
  "$appdir/usr/share/icons/hicolor/256x256/apps/$APP_ID.png"

# An AppImage needs the desktop file and its icon at the AppDir root, plus an
# AppRun entry point (here a symlink to the binary).
install -m 0644 "$ROOT/packaging/$APP_ID.desktop" "$appdir/$APP_ID.desktop"
install -m 0644 "$ROOT/packaging/icons/video-display-256px.png" "$appdir/$APP_ID.png"
ln -sf usr/bin/ddcui-rs "$appdir/AppRun"

tool="${APPIMAGETOOL:-}"
if [ -z "$tool" ]; then
  for candidate in appimagetool "$ROOT/tools/appimagetool-x86_64.AppImage"; do
    if [ -x "$candidate" ]; then
      tool="$candidate"
      break
    fi
  done
fi

if [ -n "$tool" ]; then
  out="$DIST/ddcui-rs-${VERSION}-x86_64.AppImage"
  rm -f "$out"
  APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$tool" --no-appstream "$appdir" "$out"
else
  echo "note: appimagetool not found; skipping AppImage" >&2
fi
rm -rf "$appdir"

echo "Artifacts:"
ls -lh "$DIST"
