#!/usr/bin/env bash
# dist.sh [--no-build] <out dir>
#
# Builds the release binaries and packs the prebuilt tarball that GitHub
# Releases carry and the `rustpaper-bin` AUR package installs:
#
#   rustpaper-<version>-x86_64-linux.tar.gz
#     rustpaper-<version>/bin/{rustpaper,rustpaper-selector,rustpaper-workshop}
#     rustpaper-<version>/lib/rustpaper/libsteam_api.so   (Steamworks, for rustpaper-workshop)
#     rustpaper-<version>/lib/rustpaper/rustpaper-we-helper.exe   (run inside WE's Proton)
#     rustpaper-<version>/share/applications/rustpaper.desktop
#     rustpaper-<version>/share/icons/hicolor/128x128/apps/rustpaper.png
#     rustpaper-<version>/etc/pam.d/rustpaper-lock          (the lock screen's PAM service)
#     rustpaper-<version>/LICENSE, README.md
#
# plus its .sha256. The version comes from the workspace Cargo.toml.
set -euo pipefail

build=1
[ "${1:-}" = "--no-build" ] && { build=0; shift; }
out=${1:?usage: dist.sh [--no-build] <out dir>}
repo=$(cd "$(dirname "$0")/.." && pwd)
version=$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$repo/Cargo.toml" | head -1)
bins=(rustpaper rustpaper-selector rustpaper-workshop)

if [ $build = 1 ]; then
    (cd "$repo" && cargo build --release --locked \
        -p rustpaper-engine -p rustpaper-selector -p rustpaper-workshop --bins)
    # a Windows program (needs rustup's x86_64-pc-windows-msvc target)
    (cd "$repo" && cargo build --release --locked -p rustpaper-we-helper \
        --target x86_64-pc-windows-msvc)
fi

name=rustpaper-$version
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
root=$stage/$name
install -d "$root/bin" "$root/lib/rustpaper" "$root/share/applications" "$root/share/icons/hicolor/128x128/apps"
for b in "${bins[@]}"; do install -m 755 "$repo/target/release/$b" "$root/bin/$b"; done
# rustpaper-workshop's RUNPATH has $ORIGIN/../lib/rustpaper (its build.rs)
install -m 755 "$repo/target/release/libsteam_api.so" "$root/lib/rustpaper/libsteam_api.so"
# started by the engine inside WE's Proton (web wallpapers' clicks)
install -m 644 "$repo/target/x86_64-pc-windows-msvc/release/rustpaper-we-helper.exe" \
    "$root/lib/rustpaper/rustpaper-we-helper.exe"
install -m 644 "$repo/packaging/rustpaper.desktop" "$root/share/applications/rustpaper.desktop"
# the lock screen's PAM service (/etc/pam.d/rustpaper-lock)
install -Dm644 "$repo/packaging/pam/rustpaper-lock" "$root/etc/pam.d/rustpaper-lock"
install -m 644 "$repo/crates/rustpaper-selector/icons/128x128.png" \
    "$root/share/icons/hicolor/128x128/apps/rustpaper.png"
install -m 644 "$repo/LICENSE" "$repo/README.md" "$root/"

mkdir -p "$out"
tarball=$out/$name-x86_64-linux.tar.gz
tar -C "$stage" --owner=0 --group=0 -czf "$tarball" "$name"
(cd "$out" && sha256sum "$(basename "$tarball")" > "$(basename "$tarball").sha256")
echo "$tarball"
