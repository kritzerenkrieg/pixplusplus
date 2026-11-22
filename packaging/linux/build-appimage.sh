#!/usr/bin/env bash
# Builds an AppImage of the Pix++ GUI (N1) from an already built release binary.
# Usage: build-appimage.sh BINARY APPIMAGETOOL RUNTIME OUT.AppImage
# The GUI links only libc; OpenGL, X11/Wayland and xkbcommon are loaded from the host at run
# time, and file dialogs go through the desktop portal, so nothing else is bundled.
set -euo pipefail
bin=$1 tool=$2 runtime=$3 out=$4
here=$(cd "$(dirname "$0")" && pwd)
appdir=$(mktemp -d)/Pix++.AppDir
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" "$appdir/usr/share/icons/hicolor/256x256/apps"
install -m 755 "$bin" "$appdir/usr/bin/pixplusplus-gui"
install -m 644 "$here/pixplusplus-gui.desktop" "$appdir/usr/share/applications/"
install -m 644 "$here/pixplusplus-gui.png" "$appdir/usr/share/icons/hicolor/256x256/apps/"
cp "$here/pixplusplus-gui.desktop" "$here/pixplusplus-gui.png" "$appdir/"
# README and the PixelOE licence (Apache-2.0 §4(a), G1), at the same path as in the .deb.
doc="$appdir/usr/share/doc/pixplusplus-gui"
mkdir -p "$doc"
install -m 644 "$here/../../README.md" "$here/../../LICENSES/PixelOE-Apache-2.0.txt" "$doc/"
ln -s usr/bin/pixplusplus-gui "$appdir/AppRun"
# No FUSE on CI runners: run the tool by extracting it.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$tool" --no-appstream --runtime-file "$runtime" "$appdir" "$out"
