#!/usr/bin/env bash
# install.sh [--uninstall]
#
# Builds the release binaries and installs them for the current user:
#   $PREFIX/bin/rustpaper            live wallpaper (drives Wallpaper Engine; run by
#                                    the systemd user service rustpaper.service,
#                                    which the selector writes and enables)
#   $PREFIX/bin/rustpaper-selector   gallery + customization panel
#   $PREFIX/bin/rustpaper-workshop   Steam Workshop bridge (selector's store)
#   $PREFIX/lib/rustpaper/libsteam_api.so  its Steamworks library
#   $PREFIX/share/applications/rustpaper.desktop   launcher entry
#   $PREFIX/share/icons/hicolor/128x128/apps/rustpaper.png
# PREFIX defaults to ~/.local. DESTDIR is prepended to every path (packaging).
# Runtime needs: Wallpaper Engine installed through Steam (Proton), Hyprland
# 0.56+, nsenter (util-linux), libgbm (mesa).
set -euo pipefail

PREFIX=${PREFIX:-$HOME/.local}
DEST="${DESTDIR:-}$PREFIX"
REPO=$(cd "$(dirname "$0")/.." && pwd)
BINS=(rustpaper rustpaper-selector rustpaper-workshop)
# binaries of the old native renderer, removed on install/uninstall
OLD_BINS=(rustpaper-render rustpaper-script-worker)

if [ "${1:-}" = "--uninstall" ]; then
    for b in "${BINS[@]}" "${OLD_BINS[@]}"; do rm -f "$DEST/bin/$b"; done
    rm -rf "$DEST/lib/rustpaper"
    rm -f "$DEST/share/applications/rustpaper.desktop" \
          "$DEST/share/icons/hicolor/128x128/apps/rustpaper.png"
    # the engine's systemd user service (the selector writes it)
    unit=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/rustpaper.service
    if [ -z "${DESTDIR:-}" ] && [ -f "$unit" ]; then
        systemctl --user disable --now rustpaper.service 2>/dev/null || true
        rm -f "$unit"
        systemctl --user daemon-reload 2>/dev/null || true
    fi
    echo "removed from $DEST (settings in ~/.config/rustpaper are kept)"
    exit 0
fi

(cd "$REPO" && cargo build --release -p rustpaper-engine -p rustpaper-selector -p rustpaper-workshop --bins)

install -d "$DEST/bin" "$DEST/share/applications" "$DEST/share/icons/hicolor/128x128/apps"
for b in "${BINS[@]}"; do install -m 755 "$REPO/target/release/$b" "$DEST/bin/$b"; done
for b in "${OLD_BINS[@]}"; do rm -f "$DEST/bin/$b"; done
install -Dm755 "$REPO/target/release/libsteam_api.so" "$DEST/lib/rustpaper/libsteam_api.so"
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
# An existing engine service (written by the selector) runs the new binary
# from now on; the selector rewrites the unit anyway when it differs.
unit=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/rustpaper.service
if [ -z "${DESTDIR:-}" ] && [ -f "$unit" ]; then
    sed -i "s|^ExecStart=.*|ExecStart=\"$PREFIX/bin/rustpaper\" --service|" "$unit"
    systemctl --user daemon-reload 2>/dev/null || true
    systemctl --user try-restart rustpaper.service 2>/dev/null || true
fi
echo "installed into $DEST/bin — run rustpaper-selector (or open Rustpaper from the launcher)"
