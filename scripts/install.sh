#!/usr/bin/env bash
# install.sh [--uninstall]
#
# Builds the release binaries and installs them for the current user:
#   $PREFIX/bin/rustpaper            live wallpaper engine
#   $PREFIX/bin/rustpaper-selector   gallery + customization panel
#   $PREFIX/bin/rustpaper-render     offline renderer (debugging, harness)
#   $PREFIX/share/applications/rustpaper.desktop   launcher entry
#   $PREFIX/share/icons/hicolor/128x128/apps/rustpaper.png
# PREFIX defaults to ~/.local. DESTDIR is prepended to every path (packaging).
# Runtime needs: Wallpaper Engine's assets from Steam (see README), ffmpeg
# (video wallpapers), pw-record (audio-reactive wallpapers, optional).
set -euo pipefail

PREFIX=${PREFIX:-$HOME/.local}
DEST="${DESTDIR:-}$PREFIX"
REPO=$(cd "$(dirname "$0")/.." && pwd)
BINS=(rustpaper rustpaper-selector rustpaper-render)

if [ "${1:-}" = "--uninstall" ]; then
    for b in "${BINS[@]}"; do rm -f "$DEST/bin/$b"; done
    rm -f "$DEST/share/applications/rustpaper.desktop" \
          "$DEST/share/icons/hicolor/128x128/apps/rustpaper.png"
    echo "removed from $DEST (settings in ~/.config/rustpaper are kept)"
    exit 0
fi

(cd "$REPO" && cargo build --release -p rustpaper-engine -p rustpaper-selector -p rustpaper-gl --bins)

install -d "$DEST/bin" "$DEST/share/applications" "$DEST/share/icons/hicolor/128x128/apps"
for b in "${BINS[@]}"; do install -m 755 "$REPO/target/release/$b" "$DEST/bin/$b"; done
install -m 644 "$REPO/crates/rustpaper-selector/icons/128x128.png" \
    "$DEST/share/icons/hicolor/128x128/apps/rustpaper.png"
cat > "$DEST/share/applications/rustpaper.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Rustpaper
Comment=Wallpaper Engine wallpapers on Linux
Exec=$PREFIX/bin/rustpaper-selector
Icon=rustpaper
Categories=Utility;Graphics;
DESKTOP
echo "installed into $DEST/bin — run rustpaper-selector (or open Rustpaper from the launcher)"
