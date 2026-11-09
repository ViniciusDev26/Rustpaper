#!/usr/bin/env bash
# download-testset.sh — downloads the test corpus (harness/testset.tsv) with
# steamcmd into ~/.local/share/rustpaper-testset (NOT the Steam client's
# Workshop folder: Steam deletes items you aren't subscribed to from there).
#
# One-time setup (interactive: password + Steam Guard, credentials are cached):
#   yay -S steamcmd            # or Valve's tarball
#   steamcmd +login <steam user> +quit
# Then:
#   STEAM_USER=<steam user> harness/download-testset.sh
#
# Wallpaper Engine must be owned by that account (Workshop content of a paid
# app can't be downloaded anonymously). Re-running only fetches what's missing.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
DEST="${RUSTPAPER_TESTSET_DIR:-$HOME/.local/share/rustpaper-testset}"
USER_NAME="${STEAM_USER:?set STEAM_USER to your Steam account name}"
STEAMCMD="$(command -v steamcmd || echo "$HOME/.local/share/steamcmd/steamcmd.sh")"
[ -x "$STEAMCMD" ] || { echo "steamcmd not found (yay -S steamcmd)" >&2; exit 1; }

CONTENT="$DEST/steamapps/workshop/content/431960"
mkdir -p "$DEST"
mapfile -t IDS < <(grep -v '^#' "$HERE/testset.tsv" | cut -f1)

missing() {
    for id in "${IDS[@]}"; do
        [ -f "$CONTENT/$id/project.json" ] || echo "$id"
    done
}

for attempt in 1 2 3; do
    mapfile -t TODO < <(missing)
    [ ${#TODO[@]} -eq 0 ] && break
    echo "attempt $attempt: ${#TODO[@]} item(s) to download"
    # batches keep one slow item from stalling everything
    for ((i = 0; i < ${#TODO[@]}; i += 20)); do
        args=(+force_install_dir "$DEST" +login "$USER_NAME")
        for id in "${TODO[@]:i:20}"; do
            args+=(+workshop_download_item 431960 "$id")
        done
        "$STEAMCMD" "${args[@]}" +quit || true
    done
done

mapfile -t LEFT < <(missing)
echo "done: $(( ${#IDS[@]} - ${#LEFT[@]} ))/${#IDS[@]} in $CONTENT"
[ ${#LEFT[@]} -eq 0 ] || { printf 'still missing: %s\n' "${LEFT[*]}"; exit 1; }
