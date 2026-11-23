#!/usr/bin/env bash
# pacman-repo.sh <release dir> <out dir> <gpg key fingerprint>
#
# Rustpaper's own pacman repository (the AUR's registrations were closed in
# 2026, and a repository gives `pacman -Syu` updates): builds the
# rustpaper-bin package from a release's tarball + rustpaper-bin.PKGBUILD,
# signs it, and writes a one-package repository database, all signed with
# the given key:
#
#   <out>/rustpaper-bin-<ver>-<rel>-x86_64.pkg.tar.zst (+ .sig)
#   <out>/rustpaper.db, rustpaper.files (+ .sig)   the repository
#   <out>/rustpaper.gpg                            the public key, armored
#
# The release workflow uploads them to the `pacman-repo` release
# (docs/RELEASING.md); users point pacman at its download URL (README).
# Run as a normal user (makepkg refuses root), with the key in $GNUPGHOME.
set -euo pipefail

src=$(realpath "$1")
out=$(realpath -m "$2")
key=$3
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

cp "$src"/rustpaper-*-x86_64-linux.tar.gz "$work/"
cp "$src/rustpaper-bin.PKGBUILD" "$work/PKGBUILD"
# --nodeps: the dependencies matter on the user's machine, not here (the
# package only copies prebuilt files)
(cd "$work" && PKGDEST="$out" makepkg --force --nodeps --noconfirm)

pkg=$(ls "$out"/rustpaper-bin-*-x86_64.pkg.tar.zst | sort -V | tail -1)
gpg --batch --yes --local-user "$key" --detach-sign --no-armor "$pkg"

# a fresh database with only this package: the repository keeps the
# latest version (pacman installs the newest one anyway)
rm -f "$out"/rustpaper.db* "$out"/rustpaper.files*
repo-add --sign --key "$key" "$out/rustpaper.db.tar.gz" "$pkg"
# release assets cannot be symlinks: real copies under pacman's names
for f in db files; do
    cp --remove-destination "$out/rustpaper.$f.tar.gz" "$out/rustpaper.$f"
    cp --remove-destination "$out/rustpaper.$f.tar.gz.sig" "$out/rustpaper.$f.sig"
done
gpg --batch --armor --export "$key" > "$out/rustpaper.gpg"
ls -1 "$out"
